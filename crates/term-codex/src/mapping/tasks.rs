//! Subagent threads and background terminals share the thread model used by Claude. Only descendants of our
//! thread are routed here; a second client's threads never become ours just because the server broadcasts them.

mod agents;
mod processes;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use serde_json::Value;
use term_thread::{EntryKind, Lane, TaskStatus, Update};

use super::Mapper;
use crate::Capabilities;
use crate::protocol::{Item, text};

#[derive(Debug)]
struct Agent {
    mapper: Box<Mapper>,
    caps: Capabilities,
    parent: Option<String>,
    prompt: String,
    role: Option<String>,
    started: SystemTime,
    status: TaskStatus,
    ended: Option<SystemTime>,
    tools: std::collections::BTreeSet<String>,
}

#[derive(Debug)]
struct Process {
    thread: String,
    process: String,
    item: term_thread::BackgroundItem,
}

#[derive(Debug)]
struct Early {
    thread: String,
    method: String,
    params: Value,
    at: SystemTime,
    bytes: usize,
}

#[derive(Debug, Default)]
pub(super) struct Tasks {
    agents: BTreeMap<String, Agent>,
    processes: BTreeMap<String, Process>,
    /// The latest command's turn by thread and item, so a background terminal can patch its original card.
    commands: BTreeMap<(String, String), String>,
    /// Child events can precede the parent's spawn result. Keep a bounded tail until ancestry is proven.
    early: VecDeque<Early>,
    early_bytes: usize,
}

/// A task the user can stop without stopping the main turn.
pub(crate) enum Stop {
    Agent { thread: String, turn: String },
    Process { thread: String, process: String },
}

fn agent_key(thread: &str) -> String {
    format!("codex-agent:{thread}")
}

fn prefix(thread: &str, key: &str) -> String {
    format!("child:{thread}:{key}")
}

/// Child entries get their own namespace; a reused turn/item id cannot overwrite a parent's card. Header, usage,
/// activity and todo updates belong to that child, and must never replace the main thread's state.
fn nested(thread: &str, updates: Vec<Update>) -> Vec<Update> {
    updates
        .into_iter()
        .filter_map(|update| {
            Some(match update {
                Update::Put { key, at, mut entry, .. } => {
                    match &mut entry {
                        EntryKind::ToolCall(call) => call.call_id = prefix(thread, &call.call_id),
                        EntryKind::PermissionRequest(p) => {
                            p.tool_call = p.tool_call.take().map(|k| prefix(thread, &k));
                        }
                        EntryKind::Question(q) => q.tool_call = prefix(thread, &q.tool_call),
                        _ => {}
                    }
                    Update::Put {
                        lane: Lane::Subagent(agent_key(thread)),
                        key: key.map(|k| prefix(thread, &k)),
                        at,
                        entry,
                    }
                }
                Update::Append { key, text } => Update::Append {
                    key: prefix(thread, &key),
                    text,
                },
                Update::Finish { key, text, at } => Update::Finish {
                    key: prefix(thread, &key),
                    text,
                    at,
                },
                Update::Tool { key, patch } => Update::Tool {
                    key: prefix(thread, &key),
                    patch,
                },
                Update::Resolve { key, state } => Update::Resolve {
                    key: prefix(thread, &key),
                    state,
                },
                Update::Ask { key, request_id } => Update::Ask {
                    key: prefix(thread, &key),
                    request_id,
                },
                Update::Remove { key } => Update::Remove {
                    key: prefix(thread, &key),
                },
                _ => return None,
            })
        })
        .collect()
}

impl Tasks {
    fn buffer(&mut self, thread: &str, method: &str, params: &Value, at: SystemTime) {
        let bytes = params.to_string().len();
        if bytes > super::OUTPUT_LIMIT {
            return;
        }
        while self.early.len() >= 256 || self.early_bytes + bytes > super::OUTPUT_LIMIT {
            let Some(old) = self.early.pop_front() else { break };
            self.early_bytes = self.early_bytes.saturating_sub(old.bytes);
        }
        self.early_bytes += bytes;
        self.early.push_back(Early {
            thread: thread.to_string(),
            method: method.to_string(),
            params: params.clone(),
            at,
            bytes,
        });
    }

    fn replay(&mut self, thread: &str, ours: Option<&str>) -> Vec<Update> {
        let (ready, rest): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.early)
            .into_iter()
            .partition(|e| e.thread == thread);
        self.early = rest;
        self.early_bytes = self.early.iter().map(|e| e.bytes).sum();
        let mut out = Vec::new();
        for early in ready {
            if early.method == "thread/started" {
                out.extend(self.started(&early.params, ours, early.at));
            } else {
                out.extend(self.notification(&early.method, &early.params, early.at));
            }
        }
        out
    }

    pub(super) fn item(&mut self, item: &Item, thread: Option<&str>, at: SystemTime) -> Vec<Update> {
        let Some(thread) = thread else { return Vec::new() };
        match item {
            Item::CollabAgentToolCall { .. } => self.collab(item, thread, at),
            Item::CommandExecution {
                id, status, exit_code, ..
            } => self.command_end(thread, id, status, *exit_code, at),
            _ => Vec::new(),
        }
    }

    pub(super) fn notification(&mut self, method: &str, params: &Value, at: SystemTime) -> Vec<Update> {
        let Some(thread) = text(params, "threadId") else {
            return Vec::new();
        };
        let Some(agent) = self.agents.get_mut(thread) else {
            self.buffer(thread, method, params, at);
            return Vec::new();
        };
        let (updates, _) = agent.mapper.own_notification(method, params, &mut agent.caps, at);
        let mut out = nested(thread, updates);
        out.extend(self.agent_progress(thread, method, params, at));
        if matches!(method, "item/started" | "item/completed")
            && let Some(item) = params.get("item").and_then(Item::read)
        {
            out.extend(self.item(&item, Some(thread), at));
        }
        out.extend(self.observe(method, params, at));
        out
    }

    pub(super) fn abandon(&mut self, at: SystemTime) -> Vec<Update> {
        let ids: Vec<_> = self.agents.keys().cloned().collect();
        let mut out = Vec::new();
        for id in ids {
            let agent = self.agents.get_mut(&id).expect("known agent");
            out.extend(nested(&id, agent.mapper.settle(at)));
            if !agent.status.is_final() {
                out.extend(self.agent_status(&id, TaskStatus::Stopped, None, at));
            }
        }
        for process in self.processes.values_mut() {
            let item = &mut process.item;
            item.status = TaskStatus::Stopped;
            item.ended = Some(at);
            out.push(Update::BackgroundItem(term_thread::BackgroundPatch {
                id: item.id.clone(),
                status: Some(item.status),
                ended: Some(at),
                ..term_thread::BackgroundPatch::default()
            }));
        }
        self.processes.clear();
        self.early.clear();
        self.early_bytes = 0;
        out.push(Update::Background(Vec::new()));
        out
    }
}

impl Mapper {
    pub(crate) fn knows_task_thread(&self, thread: &str) -> bool {
        self.thread.as_deref() == Some(thread) || self.tasks.agents.contains_key(thread)
    }
    pub(crate) fn end_tasks(&mut self, at: SystemTime) -> Vec<Update> {
        self.tasks.abandon(at)
    }

    pub(crate) fn task_threads(&self) -> Vec<String> {
        self.thread
            .iter()
            .cloned()
            .chain(
                self.tasks
                    .agents
                    .iter()
                    .filter(|(id, agent)| {
                        !agent.status.is_final() || self.tasks.processes.values().any(|p| &p.thread == *id)
                    })
                    .map(|(id, _)| id.clone()),
            )
            .collect()
    }

    pub(crate) fn task_stop(&self, task: &str) -> Option<Stop> {
        for (thread, agent) in &self.tasks.agents {
            if agent_key(thread) == task && !agent.status.is_final() {
                return Some(Stop::Agent {
                    thread: thread.clone(),
                    turn: agent.mapper.turn.clone()?,
                });
            }
        }
        self.tasks.processes.get(task).map(|process| Stop::Process {
            thread: process.thread.clone(),
            process: process.process.clone(),
        })
    }

    pub(crate) fn task_request(
        &mut self,
        ask: &str,
        method: &str,
        params: &Value,
        at: SystemTime,
    ) -> Option<(crate::requests::Offer, Vec<Update>)> {
        let Some(thread) = text(params, "threadId").filter(|id| Some(*id) != self.thread.as_deref()) else {
            return self.request(ask, method, params, at);
        };
        let agent = self.tasks.agents.get_mut(thread)?;
        let (mut offer, updates) = agent.mapper.request(ask, method, params, at)?;
        offer.key = prefix(thread, &offer.key);
        offer.item = offer.item.map(|k| prefix(thread, &k));
        let mut out = nested(thread, updates);
        out.push(Update::Status(term_thread::Status::RequiresAction));
        Some((offer, out))
    }
}
