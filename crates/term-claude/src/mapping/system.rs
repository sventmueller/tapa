//! The system frames: session start, status, retries, compaction, hooks, rate limits and notices.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use term_thread::{
    AgentMessage, EntryKind, ErrorKind, HookOutcome, InfoLevel, Lane, LimitStatus, Notice, RateLimit, Status,
    ToolPatch, ToolStatus, Update,
};

use super::context::context_used;
use super::{Mapper, notice, put, seconds};
use crate::protocol::{Hook, RateLimitInfo, System};

impl Mapper {
    pub(super) fn system(&mut self, s: &System, now: SystemTime) -> Vec<Update> {
        match s {
            System::Init(init) => {
                self.model = Some(init.model.clone()).filter(|m| !m.is_empty());
                let mut out = vec![Update::Session(init.session_id.clone())];
                if let Some(model) = &self.model {
                    out.push(Update::Model(model.clone()));
                }
                if !init.permission_mode.is_empty() {
                    out.push(Update::Mode(init.permission_mode.clone()));
                }
                if init.effort.is_some() {
                    out.push(Update::Effort(init.effort.clone()));
                }
                out
            }
            System::Status {
                status,
                permission_mode,
                compact_failed,
                compact_error,
            } => status_frame(
                status.as_deref(),
                permission_mode.as_ref(),
                *compact_failed,
                compact_error,
                now,
            ),
            System::ApiRetry(r) => vec![put(
                &Lane::Main,
                Some(format!("retry:{}", self.turn)),
                now,
                EntryKind::Notice(Notice::Retrying {
                    attempt: r.attempt,
                    max_attempts: r.max_retries,
                    delay: Duration::from_millis(r.retry_delay_ms),
                    error: r.error.clone(),
                    http_status: r.error_status,
                }),
            )],
            System::CompactBoundary(c) => {
                let mut out = vec![notice(
                    now,
                    Notice::Compaction {
                        manual: c.trigger == "manual",
                        tokens_before: c.pre_tokens,
                        tokens_after: c.post_tokens,
                    },
                )];
                if let Some(after) = c.post_tokens {
                    out.push(context_used(after));
                }
                out
            }
            System::TaskStarted(t) => self.task_started(t, now),
            System::TaskProgress(p) => self.task_progress(p, now),
            System::TaskUpdated(u) => self.task_updated(u, now),
            System::TaskNotification(n) => self.task_notification(n, now),
            System::BackgroundTasksChanged(tasks) => self.background_changed(tasks, now),
            System::Hook(hook) => vec![hook_notice(hook, now)],
            System::SessionStateChanged(state) => match state.as_str() {
                "idle" => vec![self.status(Status::Idle)],
                // The CLI says it runs while only background agents work after its turn ended; that is no turn the
                // user could stop, and a stop would end those agents too.
                "running" if self.busy => vec![self.status(Status::Running)],
                "running" => Vec::new(),
                "requires_action" => vec![self.status(Status::RequiresAction)],
                _ => Vec::new(),
            },
            _ => self.other_system(s, now),
        }
    }

    fn other_system(&mut self, s: &System, now: SystemTime) -> Vec<Update> {
        match s {
            System::Informational { content, level } => {
                let level = match level.as_str() {
                    "warning" => InfoLevel::Warning,
                    "suggestion" => InfoLevel::Suggestion,
                    "notice" => InfoLevel::Notice,
                    _ => InfoLevel::Info,
                };
                vec![notice(
                    now,
                    Notice::Info {
                        level,
                        text: content.clone(),
                    },
                )]
            }
            System::LocalCommandOutput(text) => vec![put(
                &Lane::Main,
                None,
                now,
                EntryKind::AgentMessage(AgentMessage {
                    markdown: text.clone(),
                    streaming: false,
                }),
            )],
            System::Notification(text) => vec![notice(
                now,
                Notice::Info {
                    level: InfoLevel::Notice,
                    text: text.clone(),
                },
            )],
            System::PermissionDenied { tool_use_id, .. } => vec![Update::Tool {
                key: tool_use_id.clone(),
                patch: ToolPatch {
                    status: Some(ToolStatus::Failed),
                    ..ToolPatch::default()
                },
            }],
            System::Refusal { content, retracted } => {
                let mut out: Vec<Update> = retracted.iter().flat_map(|uuid| self.retract(uuid)).collect();
                out.push(notice(
                    now,
                    Notice::Info {
                        level: InfoLevel::Warning,
                        text: content.clone(),
                    },
                ));
                out
            }
            System::WritesLost(code) => vec![notice(
                now,
                Notice::Error {
                    kind: ErrorKind::Disk,
                    message: code.clone(),
                },
            )],
            _ => Vec::new(),
        }
    }
}

fn status_frame(
    status: Option<&str>,
    mode: Option<&String>,
    compact_failed: bool,
    compact_error: &Option<String>,
    now: SystemTime,
) -> Vec<Update> {
    let mut out = vec![Update::Activity(match status {
        Some("compacting") => Some(term_thread::Activity::Compacting),
        Some("requesting") => Some(term_thread::Activity::Requesting),
        _ => None,
    })];
    if let Some(mode) = mode {
        out.push(Update::Mode(mode.clone()));
    }
    if compact_failed {
        out.push(notice(
            now,
            Notice::Error {
                kind: ErrorKind::Compaction,
                message: compact_error.clone().unwrap_or_default(),
            },
        ));
    }
    out
}

pub(super) fn rate_limit(info: &RateLimitInfo, now: SystemTime) -> Vec<Update> {
    let status = match info.status.as_str() {
        "allowed" => LimitStatus::Allowed,
        "rejected" => LimitStatus::Rejected,
        _ => LimitStatus::Warning,
    };
    let limit = RateLimit {
        window: info.rate_limit_type.clone().unwrap_or_else(|| "unknown".into()),
        status,
        utilization: info.utilization,
        resets_at: info.resets_at.and_then(|s| seconds(s).map(|d| UNIX_EPOCH + d)),
    };
    let mut out = vec![Update::RateLimit(limit.clone())];
    if status != LimitStatus::Allowed {
        out.push(put(
            &Lane::Main,
            Some(format!("limit:{}:{}", limit.window, info.status)),
            now,
            EntryKind::Notice(Notice::RateLimit(limit)),
        ));
    }
    out
}

fn hook_notice(hook: &Hook, now: SystemTime) -> Update {
    let outcome = match hook.outcome.as_deref() {
        None => HookOutcome::Running,
        Some("success") => HookOutcome::Success,
        Some("cancelled") => HookOutcome::Cancelled,
        Some(_) => HookOutcome::Error,
    };
    let output = if hook.output.is_empty() {
        hook.stderr.clone()
    } else {
        hook.output.clone()
    };
    put(
        &Lane::Main,
        Some(format!("hook:{}", hook.hook_id)),
        now,
        EntryKind::Notice(Notice::Hook {
            id: hook.hook_id.clone(),
            name: hook.hook_name.clone(),
            event: hook.hook_event.clone(),
            output,
            exit_code: hook.exit_code,
            outcome,
        }),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use term_thread::{Thread, ThreadId};

    use super::*;

    fn thread_after(frames: &[Value]) -> Thread {
        let mut mapper = Mapper::new();
        let mut thread = Thread::new(ThreadId::new(), "claude", "/w");
        for frame in frames {
            let message = crate::protocol::parse(&frame.to_string()).unwrap();
            thread.apply_all(mapper.map(&message, UNIX_EPOCH));
        }
        thread
    }

    fn notices(thread: &Thread) -> Vec<Notice> {
        thread
            .entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    fn info(level: InfoLevel, text: &str) -> Notice {
        Notice::Info {
            level,
            text: text.into(),
        }
    }

    #[test]
    fn notices_of_the_cli_keep_their_level_and_lost_writes_are_an_error() {
        let system = |subtype: &str, fields: Value| {
            let mut frame = json!({"type": "system", "subtype": subtype, "session_id": "s"});
            frame
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            frame
        };
        let thread = thread_after(&[
            system(
                "informational",
                json!({"content": "Try /compact", "level": "suggestion"}),
            ),
            system(
                "informational",
                json!({"content": "Using the API key", "level": "notice"}),
            ),
            system("informational", json!({"content": "Plain", "level": "other"})),
            system("notification", json!({"text": "Task finished"})),
            system("session_writes_lost", json!({"error_code": "ENOSPC"})),
            system(
                "status",
                json!({"status": null, "compact_result": "failed", "compact_error": "too long"}),
            ),
            system("session_state_changed", json!({"state": "hibernating"})),
        ]);
        assert_eq!(
            notices(&thread),
            [
                info(InfoLevel::Suggestion, "Try /compact"),
                info(InfoLevel::Notice, "Using the API key"),
                info(InfoLevel::Info, "Plain"),
                info(InfoLevel::Notice, "Task finished"),
                Notice::Error {
                    kind: ErrorKind::Disk,
                    message: "ENOSPC".into()
                },
                Notice::Error {
                    kind: ErrorKind::Compaction,
                    message: "too long".into()
                },
            ]
        );
    }

    #[test]
    fn a_denied_call_fails_and_a_refusal_takes_back_what_the_declining_model_wrote() {
        let thread = thread_after(&[
            json!({"type": "assistant", "uuid": "a-1", "session_id": "s", "parent_tool_use_id": null,
                   "message": {"id": "m1", "content": [{"type": "text", "text": "Sure, here is"}]}}),
            json!({"type": "assistant", "uuid": "a-2", "session_id": "s", "parent_tool_use_id": null,
                   "message": {"id": "m2", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "system", "subtype": "permission_denied", "tool_use_id": "toolu_1", "message": "auto mode", "session_id": "s"}),
            json!({"type": "system", "subtype": "model_refusal_fallback", "content": "Switched to another model",
                   "retracted_message_uuids": ["a-1"], "session_id": "s"}),
        ]);
        let texts: Vec<&str> = thread
            .entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::AgentMessage(m) => Some(m.markdown.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.is_empty(), "{texts:?}");
        match &thread.entry_by_key("toolu_1").map(|e| &e.kind) {
            Some(EntryKind::ToolCall(call)) => assert_eq!(call.status, ToolStatus::Failed),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            notices(&thread),
            [info(InfoLevel::Warning, "Switched to another model")]
        );
    }
}
