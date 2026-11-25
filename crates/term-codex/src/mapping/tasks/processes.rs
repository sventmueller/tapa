//! A command is background work only once Codex lists its PTY process. Output is bounded just like tool output.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use serde_json::Value;
use term_thread::{
    BackgroundItem, BackgroundKind, BackgroundPatch, OutputTail, TaskStatus, ToolPatch, ToolView, Update,
};

use super::{Process, Tasks, agent_key, prefix};
use crate::Mapper;
use crate::mapping::{OUTPUT_LIMIT, item_key};
use crate::protocol::{Item, text};

impl Tasks {
    pub(in crate::mapping) fn observe(&mut self, method: &str, params: &Value, _at: SystemTime) -> Vec<Update> {
        let Some(thread) = text(params, "threadId") else {
            return Vec::new();
        };
        if method == "item/started"
            && let Some(Item::CommandExecution { id, .. }) = params.get("item").and_then(Item::read)
            && let Some(turn) = text(params, "turnId")
        {
            let key = item_key(turn, &id);
            let key = if self.agents.contains_key(thread) {
                prefix(thread, &key)
            } else {
                key
            };
            self.commands.insert((thread.to_string(), id), key);
        }
        if method != "item/commandExecution/outputDelta" {
            return Vec::new();
        }
        let (Some(id), Some(delta)) = (text(params, "itemId"), text(params, "delta")) else {
            return Vec::new();
        };
        let call = if self.agents.contains_key(thread) {
            prefix(thread, id)
        } else {
            id.to_string()
        };
        self.processes
            .values_mut()
            .filter(|p| p.thread == thread && p.item.tool_call.as_deref() == Some(&call))
            .map(|p| {
                let tail = &mut p.item.output;
                tail.total_bytes = Some(tail.total_bytes.unwrap_or_default().saturating_add(delta.len() as u64));
                let output = tail.text.get_or_insert_with(String::new);
                output.push_str(delta);
                if output.len() > OUTPUT_LIMIT {
                    let mut cut = output.len() - OUTPUT_LIMIT;
                    while !output.is_char_boundary(cut) {
                        cut += 1;
                    }
                    output.drain(..cut);
                    tail.truncated = true;
                }
                Update::BackgroundItem(BackgroundPatch {
                    id: p.item.id.clone(),
                    output: Some(tail.clone()),
                    ..BackgroundPatch::default()
                })
            })
            .collect()
    }

    pub(super) fn command_end(
        &mut self,
        thread: &str,
        id: &str,
        state: &str,
        code: Option<i64>,
        at: SystemTime,
    ) -> Vec<Update> {
        if !matches!(state, "completed" | "failed" | "declined" | "interrupted") {
            return Vec::new();
        }
        self.commands.remove(&(thread.to_string(), id.to_string()));
        let call = if self.agents.contains_key(thread) {
            prefix(thread, id)
        } else {
            id.to_string()
        };
        let keys: Vec<_> = self
            .processes
            .iter()
            .filter(|(_, p)| p.thread == thread && p.item.tool_call.as_deref() == Some(&call))
            .map(|(key, _)| key.clone())
            .collect();
        let status = match state {
            "interrupted" => TaskStatus::Stopped,
            "failed" | "declined" => TaskStatus::Failed,
            _ if code.is_some_and(|c| c != 0) => TaskStatus::Failed,
            _ => TaskStatus::Completed,
        };
        let mut out = Vec::new();
        for key in keys {
            out.extend(self.process_end(&key, status, at));
        }
        out
    }

    pub(super) fn process_end(&mut self, key: &str, status: TaskStatus, at: SystemTime) -> Vec<Update> {
        let Some(_) = self.processes.remove(key) else {
            return Vec::new();
        };
        vec![
            Update::BackgroundItem(BackgroundPatch {
                id: key.to_string(),
                status: Some(status),
                ended: Some(at),
                ..BackgroundPatch::default()
            }),
            self.process_set(),
        ]
    }

    fn process_set(&self) -> Update {
        Update::Background(self.processes.values().map(|p| p.item.clone()).collect())
    }

    fn terminals(
        &mut self,
        thread: &str,
        terminals: &[Value],
        outputs: &BTreeMap<String, String>,
        at: SystemTime,
    ) -> Vec<Update> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for terminal in terminals {
            let (Some(process), Some(id), Some(command)) = (
                text(terminal, "processId"),
                text(terminal, "itemId"),
                text(terminal, "command"),
            ) else {
                continue;
            };
            let key = format!("codex-process:{thread}:{process}");
            seen.insert(key.clone());
            let child = self.agents.contains_key(thread);
            self.processes.entry(key.clone()).or_insert_with(|| Process {
                thread: thread.to_string(),
                process: process.to_string(),
                item: BackgroundItem {
                    id: key.clone(),
                    kind: BackgroundKind::Shell,
                    label: command.to_string(),
                    status: TaskStatus::Running,
                    started: Some(at),
                    ended: None,
                    tool_call: Some(if child { prefix(thread, id) } else { id.to_string() }),
                    parent: child.then(|| agent_key(thread)),
                    output: outputs.get(id).map_or_else(OutputTail::default, |output| OutputTail {
                        text: Some(output.clone()),
                        total_bytes: Some(output.len() as u64),
                        ..OutputTail::default()
                    }),
                    is_backgrounded: true,
                    ambient: false,
                },
            });
            if let Some(card) = self.commands.get(&(thread.to_string(), id.to_string())) {
                out.push(Update::Tool {
                    key: card.clone(),
                    patch: ToolPatch {
                        background_task: Some(key),
                        view: Some(ToolView::Command {
                            command: command.to_string(),
                            description: None,
                            background: true,
                            timeout: None,
                        }),
                        ..ToolPatch::default()
                    },
                });
            }
        }
        let gone: Vec<_> = self
            .processes
            .iter()
            .filter(|(key, p)| p.thread == thread && !seen.contains(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            out.extend(self.process_end(&key, TaskStatus::Completed, at));
        }
        out.push(self.process_set());
        out
    }
}

impl Mapper {
    pub(crate) fn background_terminals(&mut self, thread: &str, terminals: &[Value], at: SystemTime) -> Vec<Update> {
        if self.thread.as_deref() != Some(thread) && !self.tasks.agents.contains_key(thread) {
            return Vec::new();
        }
        let child = self.tasks.agents.get(thread);
        let outputs = terminals
            .iter()
            .filter_map(|terminal| {
                let id = text(terminal, "itemId")?;
                let key = self.tasks.commands.get(&(thread.to_string(), id.to_string()))?;
                let open = if let Some(child) = child {
                    child.mapper.open.get(key.strip_prefix(&format!("child:{thread}:"))?)
                } else {
                    self.open.get(key)
                };
                match open {
                    Some(crate::mapping::Open::Tool { output }) => Some((id.to_string(), output.clone())),
                    _ => None,
                }
            })
            .collect();
        self.tasks.terminals(thread, terminals, &outputs, at)
    }

    pub(crate) fn process_stopped(&mut self, task: &str, at: SystemTime) -> Vec<Update> {
        self.tasks.process_end(task, TaskStatus::Stopped, at)
    }
}
