//! Subagent identity comes from thread ancestry, not from the name of a tool such as `wait` or `sendInput`.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::Value;
use term_thread::{EntryKind, Lane, SubagentPatch, TaskStatus, ToolCall, ToolKind, ToolStatus, ToolView, Update};

use super::{Agent, Tasks, agent_key};
use crate::Mapper;
use crate::protocol::{Item, number, text};

fn status(name: &str) -> Option<TaskStatus> {
    Some(match name {
        "pendingInit" => TaskStatus::Pending,
        "active" | "running" | "inProgress" => TaskStatus::Running,
        "idle" | "completed" => TaskStatus::Completed,
        "systemError" | "errored" | "notFound" | "failed" => TaskStatus::Failed,
        "interrupted" | "shutdown" => TaskStatus::Stopped,
        _ => return None,
    })
}

impl Tasks {
    fn register(&mut self, id: &str, parent: Option<String>, prompt: &str, at: SystemTime) -> Vec<Update> {
        let agent = self.agents.entry(id.to_string()).or_insert_with(|| {
            let mut mapper = Mapper::default();
            mapper.set_thread(id);
            Agent {
                mapper: Box::new(mapper),
                caps: crate::Capabilities::default(),
                parent,
                prompt: prompt.to_string(),
                role: None,
                started: at,
                status: TaskStatus::Pending,
                ended: None,
                tools: BTreeSet::new(),
            }
        });
        if !prompt.is_empty() {
            agent.prompt = prompt.to_string();
        }
        let mut out = self.agent_card(id, at);
        out.extend(self.replay(id, None));
        out
    }

    pub(in crate::mapping) fn started(&mut self, params: &Value, ours: Option<&str>, at: SystemTime) -> Vec<Update> {
        let thread = &params["thread"];
        let (Some(id), Some(parent)) = (
            text(thread, "id"),
            text(thread, "parentThreadId").or_else(|| {
                thread
                    .pointer("/source/subAgent/thread_spawn/parent_thread_id")
                    .and_then(Value::as_str)
            }),
        ) else {
            return Vec::new();
        };
        if Some(parent) != ours && !self.agents.contains_key(parent) {
            self.buffer(parent, "thread/started", params, at);
            return Vec::new();
        }
        let parent = self.agents.contains_key(parent).then(|| agent_key(parent));
        let mut out = self.register(id, parent, "", at);
        if let Some(agent) = self.agents.get_mut(id) {
            agent.role = text(thread, "agentRole")
                .or_else(|| text(thread, "agentNickname"))
                .map(str::to_string);
        }
        out.extend(self.agent_card(id, at));
        out
    }

    pub(super) fn collab(&mut self, item: &Item, sender: &str, at: SystemTime) -> Vec<Update> {
        let Item::CollabAgentToolCall {
            tool,
            prompt,
            receiver_thread_ids,
            agents_states,
            ..
        } = item
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if tool == "spawnAgent" {
            for id in receiver_thread_ids {
                let parent = self.agents.contains_key(sender).then(|| agent_key(sender));
                out.extend(self.register(id, parent, prompt.as_deref().unwrap_or_default(), at));
            }
        }
        for (id, state) in agents_states {
            if let Some(status) = status(&state.status) {
                if status == TaskStatus::Running
                    && self
                        .agents
                        .get(id)
                        .is_some_and(|a| a.status.is_final() && a.mapper.turn().is_none())
                {
                    continue;
                }
                out.extend(self.agent_status(id, status, state.message.clone(), at));
            }
        }
        out
    }

    fn agent_card(&self, id: &str, at: SystemTime) -> Vec<Update> {
        let Some(agent) = self.agents.get(id) else {
            return Vec::new();
        };
        let key = agent_key(id);
        let mut call = ToolCall::new(key.clone(), "Agent", ToolKind::Think);
        call.title = term_thread::title_from(&agent.prompt);
        call.status = match agent.status {
            TaskStatus::Completed => ToolStatus::Completed,
            TaskStatus::Failed | TaskStatus::Stopped => ToolStatus::Failed,
            _ => ToolStatus::InProgress,
        };
        call.started = Some(agent.started);
        call.ended = agent.ended;
        call.view = ToolView::Agent {
            description: call.title.clone(),
            agent_type: agent.role.clone(),
            prompt: agent.prompt.clone(),
            background: true,
        };
        let mut patch = SubagentPatch::new(key.clone(), agent.started);
        patch.task_id = Some(key.clone());
        patch.description = Some(call.title.clone());
        patch.agent_type = agent.role.clone();
        patch.parent = agent.parent.clone();
        patch.backgrounded = Some(true);
        patch.status = Some(agent.status);
        patch.cancellable = Some(agent.mapper.turn().is_some() && !agent.status.is_final());
        patch.ended = call.ended;
        patch.elapsed = Some(
            agent
                .ended
                .unwrap_or(at)
                .duration_since(agent.started)
                .unwrap_or_default(),
        );
        vec![
            Update::Put {
                lane: agent.parent.clone().map_or(Lane::Main, Lane::Subagent),
                key: Some(key),
                at: agent.started,
                entry: EntryKind::ToolCall(call),
            },
            Update::Subagent(patch),
        ]
    }

    pub(super) fn agent_status(
        &mut self,
        id: &str,
        status: TaskStatus,
        summary: Option<String>,
        at: SystemTime,
    ) -> Vec<Update> {
        let Some(agent) = self.agents.get_mut(id) else {
            return Vec::new();
        };
        if status.is_final() {
            agent.ended.get_or_insert(at);
        } else {
            agent.ended = None;
        }
        agent.status = status;
        let mut out = self.agent_card(id, at);
        if let Some(summary) = summary.filter(|s| !s.is_empty()) {
            let mut patch = SubagentPatch::new(agent_key(id), at);
            patch.summary = Some(summary);
            out.push(Update::Subagent(patch));
        }
        out
    }

    pub(super) fn agent_progress(&mut self, id: &str, method: &str, params: &Value, at: SystemTime) -> Vec<Update> {
        let mut patch = SubagentPatch::new(agent_key(id), at);
        let mut out = Vec::new();
        match method {
            "turn/started" => out.extend(self.agent_status(id, TaskStatus::Running, None, at)),
            "turn/completed" => {
                let state = text(&params["turn"], "status")
                    .and_then(status)
                    .unwrap_or(TaskStatus::Completed);
                out.extend(self.agent_status(id, state, None, at));
            }
            "thread/status/changed" => {
                if let Some(state) = text(&params["status"], "type").and_then(status) {
                    out.extend(self.agent_status(id, state, None, at));
                }
            }
            "thread/closed" => out.extend(self.agent_status(id, TaskStatus::Stopped, None, at)),
            "thread/tokenUsage/updated" => patch.tokens = number(&params["tokenUsage"]["total"], "totalTokens"),
            "item/completed" if text(&params["item"], "type") == Some("agentMessage") => {
                patch.summary = text(&params["item"], "text").map(str::to_string);
            }
            "item/started" => {
                if let Some(item) = params.get("item").and_then(Item::read)
                    && let Some(card) = super::super::cards::card(&item)
                    && let Some(agent) = self.agents.get_mut(id)
                {
                    agent
                        .tools
                        .insert(format!("{}:{}", text(params, "turnId").unwrap_or_default(), card.id));
                    patch.tool_uses = Some(agent.tools.len() as u64);
                    patch.last_tool = Some(card.name);
                }
            }
            _ => {}
        }
        out.push(Update::Subagent(patch));
        out
    }
}
