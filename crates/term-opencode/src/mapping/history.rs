//! The conversation of a resumed session, as `GET /api/session/{id}/message` lists its `Session.Message.Info`s.

use std::time::SystemTime;

use serde_json::Value;
use term_thread::{AgentMessage, EntryKind, ErrorKind, Notice, Thought, ToolCall, ToolStatus, Update, UserMessage};

use super::{Mapper, cards, error_kind, error_message, millis, put, text, tool_key, usage};

impl Mapper {
    /// The thread's entries for `messages`, oldest first: what the thread showed goes first.
    pub(crate) fn history(&mut self, messages: &[Value], at: SystemTime) -> Vec<Update> {
        let mut out = vec![Update::Clear];
        for message in messages {
            let Some(id) = text(message, "id") else {
                continue;
            };
            let time = message.get("time").unwrap_or(&Value::Null);
            let created = millis(time, "created", at);
            match text(message, "type") {
                Some("user") => {
                    let user = UserMessage {
                        text: text(message, "text").unwrap_or_default().to_string(),
                        message_id: Some(id.to_string()),
                        ..UserMessage::default()
                    };
                    out.push(put(format!("user:{id}"), created, EntryKind::UserMessage(user)));
                    out.push(super::chain(format!("user:{id}"), id));
                }
                Some("assistant") => out.extend(self.assistant(message, id, created)),
                Some("compaction") if text(message, "status") == Some("completed") => {
                    let notice = Notice::Compaction {
                        manual: text(message, "reason") == Some("manual"),
                        tokens_before: message
                            .get("tokens")
                            .map(|t| usage::tokens(t).total())
                            .unwrap_or_default(),
                        tokens_after: None,
                    };
                    out.push(put(format!("compaction:{id}"), created, EntryKind::Notice(notice)));
                }
                Some("idle") => out.extend(self.idle(message, id, created)),
                Some("shell") => out.push(shell(message, id, created)),
                _ => {}
            }
        }
        out
    }

    fn assistant(&mut self, message: &Value, id: &str, created: SystemTime) -> Vec<Update> {
        let completed = millis(message.get("time").unwrap_or(&Value::Null), "completed", created);
        let mut out = Vec::new();
        for (n, part) in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let body = text(part, "text").unwrap_or_default().to_string();
            match text(part, "type") {
                Some("text") => {
                    let entry = EntryKind::AgentMessage(AgentMessage {
                        markdown: body,
                        streaming: false,
                    });
                    out.push(put(format!("text:{id}:h{n}"), created, entry));
                }
                Some("reasoning") => {
                    let time = part.get("time").unwrap_or(&Value::Null);
                    let key = format!("reasoning:{id}:h{n}");
                    let thought = Thought {
                        text: body.clone(),
                        streaming: true,
                        ..Thought::default()
                    };
                    out.push(put(
                        key.clone(),
                        millis(time, "created", created),
                        EntryKind::Thought(thought),
                    ));
                    out.push(Update::Finish {
                        key,
                        text: Some(body),
                        at: millis(time, "completed", completed),
                    });
                }
                Some("tool") => out.extend(self.stored_tool(part, id, created)),
                _ => {}
            }
        }
        if let Some(error) = message.get("error").filter(|e| e.is_object()) {
            out.extend(self.error(error_kind(error), error_message(error), completed));
        }
        out
    }

    fn stored_tool(&mut self, part: &Value, message: &str, created: SystemTime) -> Vec<Update> {
        let (Some(call_id), Some(name)) = (text(part, "id"), text(part, "name")) else {
            return Vec::new();
        };
        let state = part.get("state").unwrap_or(&Value::Null);
        let input = state
            .get("input")
            .filter(|i| i.is_object())
            .cloned()
            .unwrap_or(Value::Null);
        let card = cards::card_for(name, &input);
        let time = part.get("time").unwrap_or(&Value::Null);
        let mut call = ToolCall::new(call_id, card.name, card.kind);
        call.title = card.title;
        call.view = card.view;
        call.started = Some(millis(time, "created", created));
        // A stored call that never settled was cut off by the end of its turn.
        call.status = if text(state, "status") == Some("completed") {
            ToolStatus::Completed
        } else {
            ToolStatus::Failed
        };
        call.ended = Some(millis(time, "completed", created));
        call.output = if call.status == ToolStatus::Failed {
            let error = state.get("error").unwrap_or(&Value::Null);
            Some(term_thread::ToolOutput::Text(
                cards::content_text(state.get("content")).unwrap_or_else(|| error_message(error)),
            ))
        } else {
            cards::output(name, &input, state.get("content"), state.get("metadata"))
        };
        let key = tool_key(message, call_id);
        if name == "todowrite" {
            return cards::todos(&input)
                .map(|list| {
                    vec![
                        Update::Todos(list.clone()),
                        put(format!("todos:{key}"), created, EntryKind::Todos(list)),
                    ]
                })
                .unwrap_or_default();
        }
        call.input = input;
        vec![put(key, call.started.unwrap_or(created), EntryKind::ToolCall(call))]
    }

    fn idle(&mut self, message: &Value, id: &str, created: SystemTime) -> Vec<Update> {
        match text(message, "outcome") {
            Some("interrupted") => vec![put(
                format!("idle:{id}"),
                created,
                EntryKind::Notice(Notice::Interrupted),
            )],
            Some("failed") => {
                let error = message.get("error").unwrap_or(&Value::Null);
                let message = Some(error_message(error))
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| "failed".into());
                let notice = Notice::Error {
                    kind: if error.is_null() {
                        ErrorKind::Turn
                    } else {
                        error_kind(error)
                    },
                    message,
                };
                vec![put(format!("idle:{id}"), created, EntryKind::Notice(notice))]
            }
            _ => Vec::new(),
        }
    }
}

/// A shell command the user ran in the session, as the card a command of the agent gets.
fn shell(message: &Value, id: &str, created: SystemTime) -> Update {
    let input = serde_json::json!({"command": text(message, "command").unwrap_or_default()});
    let card = cards::card_for("shell", &input);
    let mut call = ToolCall::new(id, card.name, card.kind);
    call.title = card.title;
    call.view = card.view;
    call.input = input;
    call.status = match text(message, "status") {
        Some("exited") => ToolStatus::Completed,
        Some("running") => ToolStatus::InProgress,
        _ => ToolStatus::Failed,
    };
    call.output =
        message
            .get("output")
            .and_then(|o| text(o, "output"))
            .map(|stdout| term_thread::ToolOutput::Terminal {
                stdout: stdout.to_string(),
                stderr: String::new(),
                interrupted: false,
            });
    call.started = Some(created);
    put(format!("shell:{id}"), created, EntryKind::ToolCall(call))
}
