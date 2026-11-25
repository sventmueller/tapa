//! The agent's frames: its text, thoughts and tool calls.

use std::time::SystemTime;

use serde_json::Value;
use term_thread::{
    AgentMessage, EntryKind, ErrorKind, Lane, Notice, SubagentPatch, TaskStatus, Thought, ToolCall, ToolPatch,
    ToolStatus, Update,
};

use super::context::context_used;
use super::requests::{plan, question};
use super::{Mapper, Slot, Tool, lane, notice, put};
use crate::protocol::{Assistant, Block};
use crate::tools::{self, Shape};

/// The errors on assistant frames that mean the CLI is not signed in or its sign-in was refused.
const AUTH_ERRORS: [&str; 5] = [
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "verification_required",
    "cloud_credential_error",
];

impl Mapper {
    pub(super) fn assistant(&mut self, a: &Assistant, now: SystemTime) -> Vec<Update> {
        let lane = lane(a.parent_tool_use_id.as_ref());
        if let Some(error) = &a.error {
            return self.assistant_error(error, a, now);
        }
        let mut out = Vec::new();
        let mut keys = Vec::new();
        let id = &a.message.id;
        for block in &a.message.content {
            match block {
                Block::Text { text } => keys.push(self.text_block(&lane, id, Slot::Text, text, now, &mut out)),
                Block::Thinking { thinking } => {
                    keys.push(self.text_block(&lane, id, Slot::Thought, thinking, now, &mut out));
                }
                Block::RedactedThinking => keys.push(self.text_block(&lane, id, Slot::Thought, "", now, &mut out)),
                Block::ToolUse { id, name, input } | Block::ServerToolUse { id, name, input } => {
                    self.tool_use(
                        &lane,
                        a.parent_tool_use_id.as_ref(),
                        id,
                        name,
                        input,
                        true,
                        now,
                        &mut out,
                    );
                    keys.push(id.clone());
                }
                _ => {}
            }
        }
        for gone in &a.supersedes {
            out.extend(self.retract(gone));
        }
        if let Some(uuid) = &a.uuid {
            if lane == Lane::Main {
                out.extend(self.link(&keys, uuid));
            }
            self.frames.insert(uuid.clone(), keys);
        }
        if lane == Lane::Main
            && !self.streams.get(&lane).is_some_and(|s| &s.message == id)
            && let Some(usage) = &a.message.usage
        {
            out.push(context_used(usage.context_input() + usage.output_tokens.unwrap_or(0)));
        }
        out
    }

    fn assistant_error(&mut self, error: &str, a: &Assistant, now: SystemTime) -> Vec<Update> {
        let text: Vec<&str> = a
            .message
            .content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let kind = if AUTH_ERRORS.contains(&error) {
            self.auth_reported = true;
            ErrorKind::Auth
        } else {
            ErrorKind::Api
        };
        let message = if text.is_empty() {
            error.to_string()
        } else {
            text.join("\n")
        };
        vec![notice(now, Notice::Error { kind, message })]
    }

    pub(super) fn retract(&mut self, uuid: &str) -> Vec<Update> {
        self.chain.retain(|frame| frame != uuid);
        self.frames
            .remove(uuid)
            .unwrap_or_default()
            .into_iter()
            .map(|key| Update::Remove { key })
            .collect()
    }

    fn text_block(
        &mut self,
        lane: &Lane,
        message: &str,
        slot: Slot,
        text: &str,
        now: SystemTime,
        out: &mut Vec<Update>,
    ) -> String {
        let streamed = self
            .streamed
            .get_mut(message)
            .and_then(|list| list.iter_mut().find(|s| !s.claimed && s.slot == slot));
        if let Some(streamed) = streamed {
            streamed.claimed = true;
            let key = streamed.key.clone();
            self.open.retain(|k| k != &key);
            out.push(Update::Finish {
                key: key.clone(),
                text: Some(text.to_string()),
                at: now,
            });
            return key;
        }
        self.unnamed += 1;
        let key = format!("{message}:f{}", self.unnamed);
        let entry = match slot {
            Slot::Text => EntryKind::AgentMessage(AgentMessage {
                markdown: text.to_string(),
                streaming: false,
            }),
            Slot::Thought => EntryKind::Thought(Thought {
                text: text.to_string(),
                duration: None,
                streaming: false,
            }),
        };
        out.push(put(lane, Some(key.clone()), now, entry));
        key
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one call site per frame kind, all fields are needed"
    )]
    pub(super) fn tool_use(
        &mut self,
        lane: &Lane,
        parent: Option<&String>,
        id: &str,
        name: &str,
        input: &Value,
        complete: bool,
        now: SystemTime,
        out: &mut Vec<Update>,
    ) {
        let known = self.tools.contains_key(id);
        self.tools.insert(
            id.to_string(),
            Tool {
                name: name.to_string(),
                lane: lane.clone(),
            },
        );
        let request = self.tool_requests.get(id).map(String::as_str);
        match tools::shape(name) {
            Shape::Todos => {
                let list = tools::todos(input);
                if complete && *lane == Lane::Main {
                    out.push(Update::Todos(list.clone()));
                }
                out.push(put(lane, Some(id.to_string()), now, EntryKind::Todos(list)));
            }
            Shape::Question => out.push(put(lane, Some(id.to_string()), now, question(id, input, request))),
            Shape::Plan => out.push(put(lane, Some(id.to_string()), now, plan(id, input, request))),
            Shape::Card => {
                let (kind, title, view) = tools::describe(name, input);
                let status = if complete {
                    ToolStatus::InProgress
                } else {
                    ToolStatus::Pending
                };
                if complete {
                    self.running.insert(id.to_string());
                }
                if known {
                    out.push(Update::Tool {
                        key: id.to_string(),
                        patch: ToolPatch {
                            input: Some(input.clone()),
                            title: Some(title),
                            view: Some(view),
                            status: Some(status),
                            started: complete.then_some(now),
                            ..ToolPatch::default()
                        },
                    });
                } else {
                    let mut call = ToolCall::new(id, name, kind);
                    call.title = title;
                    call.view = view;
                    call.input = input.clone();
                    call.status = status;
                    call.started = complete.then_some(now);
                    call.parent = parent.cloned();
                    out.push(put(lane, Some(id.to_string()), now, EntryKind::ToolCall(call)));
                }
                if complete && tools::starts_subagent(name) {
                    out.push(self.subagent_from_call(id, input, now));
                }
            }
        }
    }

    fn subagent_from_call(&mut self, id: &str, input: &Value, now: SystemTime) -> Update {
        let text = |f: &str| input.get(f).and_then(Value::as_str).map(str::to_string);
        self.live_subagents.insert(id.to_string());
        let mut patch = SubagentPatch::new(id, now);
        patch.description = text("description");
        patch.agent_type = text("subagent_type");
        patch.status = Some(TaskStatus::Running);
        patch.backgrounded = input.get("run_in_background").and_then(Value::as_bool);
        Update::Subagent(patch)
    }
}

/// The CLI's own words when `a` reports a missing or refused sign-in.
pub(crate) fn sign_in_problem(a: &Assistant) -> Option<String> {
    let error = a.error.as_deref()?;
    if !AUTH_ERRORS.contains(&error) {
        return None;
    }
    let text: Vec<&str> = a
        .message
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    Some(if text.is_empty() {
        error.to_string()
    } else {
        text.join("\n")
    })
}
