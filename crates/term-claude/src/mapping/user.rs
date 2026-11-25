//! The user-role frames: tool results, the user's messages and their echoes.

use std::time::{Duration, SystemTime};

use serde_json::Value;
use term_thread::{
    Decision, EntryKind, Lane, RequestState, SubagentPatch, TaskStatus, ToolPatch, ToolStatus, Update, UserMessage,
};

use super::tasks::{notification_in, task_status};
use super::{Mapper, Task, lane, put};
use crate::control;
use crate::protocol::{Block, ImageSource, User};
use crate::tools::{self, Shape};

impl Mapper {
    pub(super) fn user(&mut self, u: &User, now: SystemTime) -> Vec<Update> {
        if u.is_replay == Some(true) {
            let mut out: Vec<Update> = u
                .message
                .blocks()
                .into_iter()
                .filter_map(|block| match block {
                    Block::Text { text } => notification_in(&text),
                    _ => None,
                })
                .flat_map(|notification| self.task_notification(&notification, now))
                .collect();
            out.extend(self.prompt_from_elsewhere(u, now));
            return out;
        }
        let lane = lane(u.parent_tool_use_id.as_ref());
        let mut out = Vec::new();
        let mut text = Vec::new();
        let mut images = Vec::new();
        let mut keys = Vec::new();
        for block in u.message.blocks() {
            match block {
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    self.tool_result(
                        &tool_use_id,
                        &content,
                        is_error == Some(true),
                        u.tool_use_result.as_ref(),
                        now,
                        &mut out,
                    );
                    keys.push(tool_use_id);
                }
                Block::Text { text: t } => match notification_in(&t) {
                    Some(notification) => out.extend(self.task_notification(&notification, now)),
                    None => text.push(t),
                },
                Block::Image { source: Some(source) } => images.extend(image(&source)),
                _ => {}
            }
        }
        let wanted = self.history || lane != Lane::Main;
        if wanted && !(text.is_empty() && images.is_empty()) && u.is_synthetic != Some(true) {
            let key = u.uuid.as_ref().map(|id| format!("user:{id}"));
            let message = UserMessage {
                text: text.join("\n"),
                attachments: images,
                message_id: u.uuid.clone(),
            };
            keys.extend(key.clone());
            out.push(put(&lane, key, now, EntryKind::UserMessage(message)));
        }
        // Every user-role frame of the main conversation is part of its chain, also those the timeline leaves out.
        if lane == Lane::Main
            && let Some(uuid) = &u.uuid
        {
            out.extend(self.link(&keys, uuid));
        }
        out
    }

    /// The echo of a prompt the app did not send, such as one typed on claude.ai through Remote Control, as the user's
    /// message; echoes of the app's own prompts, of the CLI's own and of task notifications are left out.
    fn prompt_from_elsewhere(&mut self, u: &User, now: SystemTime) -> Vec<Update> {
        let Some(uuid) = u.uuid.as_deref() else {
            return Vec::new();
        };
        let person = u
            .origin
            .as_ref()
            .is_none_or(|o| matches!(o.get("kind").and_then(Value::as_str), Some("human" | "remote")));
        if self.history
            || self.sent.contains(uuid)
            || u.parent_tool_use_id.is_some()
            || u.is_synthetic == Some(true)
            || !person
        {
            return Vec::new();
        }
        let mut text = Vec::new();
        let mut images = Vec::new();
        for block in u.message.blocks() {
            match block {
                Block::Text { text: t } => text.push(t),
                Block::Image { source: Some(source) } => images.extend(image(&source)),
                _ => {}
            }
        }
        let text = text.join("\n");
        if text.trim_start().starts_with('<') || (text.trim().is_empty() && images.is_empty()) {
            return Vec::new();
        }
        let message = UserMessage {
            text,
            attachments: images,
            message_id: None,
        };
        self.user_message(message, uuid, now)
    }

    fn tool_result(
        &mut self,
        id: &str,
        content: &Value,
        failed: bool,
        structured: Option<&Value>,
        now: SystemTime,
        out: &mut Vec<Update>,
    ) {
        self.running.remove(id);
        let name = self.tools.get(id).map(|t| t.name.clone()).unwrap_or_default();
        match tools::shape(&name) {
            Shape::Todos => {}
            Shape::Question => {
                let answers: Vec<term_thread::Answer> = structured
                    .and_then(|s| s.get("answers"))
                    .and_then(Value::as_object)
                    .map(|map| {
                        map.iter()
                            .map(|(q, a)| term_thread::Answer {
                                question: q.clone(),
                                answer: a.as_str().map_or_else(|| a.to_string(), str::to_string),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let decision = if failed {
                    Decision::Denied {
                        message: Some(tools::result_text(content)),
                    }
                } else {
                    Decision::Answered(answers)
                };
                out.push(Update::Resolve {
                    key: id.to_string(),
                    state: RequestState::Decided(decision),
                });
            }
            // The user's own answer stands; only a session rebuilt from a transcript reads it back from the result.
            Shape::Plan if self.answered.remove(id) => {}
            Shape::Plan => {
                let decision = if failed {
                    let text = tools::result_text(content);
                    control::plan_rejection(Some(&text), text.contains(control::PLAN_REJECT_MESSAGE))
                } else {
                    Decision::Approved
                };
                out.push(Update::Resolve {
                    key: id.to_string(),
                    state: RequestState::Decided(decision),
                });
            }
            Shape::Card => {
                out.push(Update::Tool {
                    key: id.to_string(),
                    patch: ToolPatch {
                        status: Some(if failed {
                            ToolStatus::Failed
                        } else {
                            ToolStatus::Completed
                        }),
                        output: Some(tools::output(&name, structured, content)),
                        ended: Some(now),
                        results: tools::result_count(&name, structured),
                        background_task: tools::background_task(structured),
                        ..ToolPatch::default()
                    },
                });
                if tools::starts_subagent(&name)
                    && let Some(update) = self.subagent_result(id, failed, structured, now)
                {
                    out.push(update);
                }
            }
        }
    }

    fn subagent_result(
        &mut self,
        id: &str,
        failed: bool,
        structured: Option<&Value>,
        now: SystemTime,
    ) -> Option<Update> {
        let result = structured.unwrap_or(&Value::Null);
        if result.get("status").and_then(Value::as_str) == Some("async_launched") {
            return self.async_launched(id, result, now);
        }
        let status = match (failed, result.get("status").and_then(Value::as_str)) {
            (true, _) => TaskStatus::Failed,
            (false, Some("completed")) => TaskStatus::Completed,
            (false, Some(other)) => task_status(other)?,
            (false, None) => TaskStatus::Completed,
        };
        let number = |f: &str| result.get(f).and_then(Value::as_u64);
        self.live_subagents.remove(id);
        let mut patch = SubagentPatch::new(id, now);
        patch.status = Some(status);
        patch.ended = Some(now);
        patch.tokens = number("totalTokens");
        patch.tool_uses = number("totalToolUseCount");
        patch.elapsed = number("totalDurationMs").map(Duration::from_millis);
        patch.cancellable = Some(false);
        Some(Update::Subagent(patch))
    }

    /// An agent the call started in the background: its id is the task id that its end names, at times without
    /// the call.
    fn async_launched(&mut self, id: &str, result: &Value, now: SystemTime) -> Option<Update> {
        let task_id = result.get("agentId").and_then(Value::as_str)?;
        self.tasks.entry(task_id.to_string()).or_insert_with(|| Task {
            tool_use_id: Some(id.to_string()),
            subagent: Some(id.to_string()),
            started: now,
            output_file: None,
        });
        let mut patch = SubagentPatch::new(id, now);
        patch.task_id = Some(task_id.to_string());
        patch.backgrounded = Some(true);
        Some(Update::Subagent(patch))
    }
}

/// The picture a user frame carries, as an attachment.
pub(crate) fn image(source: &ImageSource) -> Option<term_thread::Attachment> {
    use base64::Engine;
    (source.kind == "base64").then_some(())?;
    Some(term_thread::Attachment::Image {
        name: None,
        media_type: source.media_type.clone(),
        data: base64::engine::general_purpose::STANDARD
            .decode(&source.data)
            .ok()?
            .into(),
    })
}
