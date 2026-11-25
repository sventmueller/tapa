//! The `system` frames: the session's set-up, its state, tasks, hooks and notices.

use serde::Deserialize;
use serde_json::Value;

use super::{text, typed};

/// A `system` frame, by subtype.
#[derive(Debug, Clone, PartialEq)]
pub enum System {
    /// The session's set-up.
    Init(Init),
    /// The CLI's status and permission mode.
    Status {
        /// `compacting`, `requesting` or none.
        status: Option<String>,
        /// The permission mode, when it changed.
        permission_mode: Option<String>,
        /// Whether a compaction failed.
        compact_failed: bool,
        /// Why it failed.
        compact_error: Option<String>,
    },
    /// A request failed and is retried.
    ApiRetry(ApiRetry),
    /// The conversation was compacted.
    CompactBoundary(CompactBoundary),
    /// A task started.
    TaskStarted(TaskStarted),
    /// A task's progress.
    TaskProgress(TaskProgress),
    /// Fields of a task changed.
    TaskUpdated(TaskUpdated),
    /// A task ended.
    TaskNotification(TaskNotification),
    /// The complete set of live background tasks.
    BackgroundTasksChanged(Vec<BackgroundTask>),
    /// A hook started, printed or ended.
    Hook(Hook),
    /// The session's state.
    SessionStateChanged(String),
    /// A text line to show.
    Informational {
        /// The text.
        content: String,
        /// `info`, `notice`, `suggestion` or `warning`.
        level: String,
    },
    /// What a local slash command printed.
    LocalCommandOutput(String),
    /// A notification.
    Notification(String),
    /// A tool call was denied without asking.
    PermissionDenied {
        /// The call.
        tool_use_id: String,
        /// What the model was told.
        message: String,
    },
    /// The model refused and the CLI retried or gave up.
    Refusal {
        /// The text to show.
        content: String,
        /// Frames the CLI took back.
        retracted: Vec<String>,
    },
    /// The slash commands changed.
    CommandsChanged(Vec<Value>),
    /// The disk is full and the transcript is not written.
    WritesLost(String),
    /// The Remote Control connection changed.
    BridgeState {
        /// `ready`, `connected`, `failed`, `policy_disabled` or another the app does not know.
        state: String,
        /// Which connection it speaks of.
        epoch: Option<u64>,
        /// Why it failed.
        detail: Option<String>,
    },
    /// Any other subtype.
    Unknown(String),
}

impl System {
    pub(super) fn read(value: &Value) -> Option<System> {
        let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or_default();
        let field = |name: &str| text(value, name);
        Some(match subtype {
            "init" => System::Init(typed(value)?),
            "status" => System::Status {
                status: field("status"),
                permission_mode: field("permissionMode"),
                compact_failed: field("compact_result").as_deref() == Some("failed"),
                compact_error: field("compact_error"),
            },
            "api_retry" => System::ApiRetry(typed(value)?),
            "compact_boundary" => System::CompactBoundary(typed(value.get("compact_metadata")?)?),
            "task_started" => System::TaskStarted(typed(value)?),
            "task_progress" => System::TaskProgress(typed(value)?),
            "task_updated" => System::TaskUpdated(typed(value)?),
            "task_notification" => System::TaskNotification(typed(value)?),
            "background_tasks_changed" => System::BackgroundTasksChanged(typed(value.get("tasks")?)?),
            "hook_started" | "hook_progress" | "hook_response" => {
                let mut hook: Hook = typed(value)?;
                if subtype == "hook_started" {
                    hook.outcome = None;
                } else if subtype == "hook_progress" {
                    hook.outcome = None;
                    hook.running = true;
                }
                System::Hook(hook)
            }
            "session_state_changed" => System::SessionStateChanged(field("state")?),
            "informational" => System::Informational {
                content: field("content")?,
                level: field("level").unwrap_or_default(),
            },
            "local_command_output" => System::LocalCommandOutput(field("content")?),
            "notification" => System::Notification(field("text")?),
            "permission_denied" => System::PermissionDenied {
                tool_use_id: field("tool_use_id")?,
                message: field("message").unwrap_or_default(),
            },
            "model_refusal_fallback" | "model_refusal_no_fallback" => System::Refusal {
                content: field("content").unwrap_or_default(),
                retracted: value.get("retracted_message_uuids").and_then(typed).unwrap_or_default(),
            },
            "commands_changed" => System::CommandsChanged(typed(value.get("commands")?)?),
            "session_writes_lost" => System::WritesLost(field("error_code").unwrap_or_default()),
            "bridge_state" => System::BridgeState {
                state: field("state")?,
                epoch: value.get("bridge_epoch").and_then(Value::as_u64),
                detail: field("detail"),
            },
            other => System::Unknown(other.to_string()),
        })
    }
}

/// The session's set-up, sent at the start of each turn.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct Init {
    pub(crate) session_id: String,
    pub(crate) cwd: String,
    pub(crate) model: String,
    #[serde(rename = "permissionMode")]
    pub(crate) permission_mode: String,
    /// The tools the model can call.
    pub(crate) tools: Vec<String>,
    /// The MCP servers and their state.
    pub(crate) mcp_servers: Vec<McpServer>,
    pub(crate) slash_commands: Vec<String>,
    /// The subagent types.
    pub(crate) agents: Vec<String>,
    pub(crate) skills: Vec<String>,
    pub(crate) claude_code_version: String,
    pub(crate) output_style: String,
    /// Protocol capabilities, such as `interrupt_receipt_v1`.
    pub(crate) capabilities: Vec<String>,
    /// The effort for the next message.
    pub(crate) effort: Option<String>,
    /// Where an API key came from, `none` with a subscription sign-in.
    #[serde(rename = "apiKeySource")]
    pub(crate) api_key_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct McpServer {
    pub(crate) name: String,
    /// `connected`, `pending`, `failed` or another state.
    pub(crate) status: String,
}

/// A request failed and is retried.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ApiRetry {
    /// The attempt that failed.
    #[serde(default)]
    pub(crate) attempt: u32,
    /// How many attempts are made at most.
    #[serde(default)]
    pub(crate) max_retries: u32,
    /// How long the CLI waits.
    #[serde(default)]
    pub(crate) retry_delay_ms: u64,
    /// The HTTP status, when there was a response.
    #[serde(default)]
    pub(crate) error_status: Option<u16>,
    /// The CLI's name for the error.
    #[serde(default)]
    pub(crate) error: String,
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CompactBoundary {
    /// `manual` or `auto`.
    #[serde(default)]
    pub(crate) trigger: String,
    #[serde(default)]
    pub(crate) pre_tokens: u64,
    #[serde(default)]
    pub(crate) post_tokens: Option<u64>,
}

/// A task started: a subagent, a shell, a monitor or a workflow.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskStarted {
    pub(crate) task_id: String,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    /// The tool call that started it.
    #[serde(default)]
    pub(crate) tool_use_id: Option<String>,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) subagent_type: Option<String>,
    /// Whether it started in the background.
    #[serde(default)]
    pub(crate) is_backgrounded: Option<bool>,
    /// The subagent task that started it.
    #[serde(default)]
    pub(crate) parent_task_id: Option<String>,
    /// `local_agent`, `local_bash`, `monitor`, `local_workflow` or another.
    #[serde(default)]
    pub(crate) task_type: Option<String>,
    /// Whether it stays out of the transcript.
    #[serde(default)]
    pub(crate) skip_transcript: Option<bool>,
    /// Whether it is housekeeping.
    #[serde(default)]
    pub(crate) ambient: Option<bool>,
}

/// Token, tool and time counts of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TaskUsage {
    pub(crate) total_tokens: u64,
    /// Tools called.
    pub(crate) tool_uses: u64,
    pub(crate) duration_ms: u64,
}

/// A task's progress.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskProgress {
    pub(crate) task_id: String,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    /// The tool call that started it.
    #[serde(default)]
    pub(crate) tool_use_id: Option<String>,
    /// What it does now.
    #[serde(default)]
    pub(crate) description: Option<String>,
    /// Counts so far.
    #[serde(default)]
    pub(crate) usage: TaskUsage,
    /// The tool it called last.
    #[serde(default)]
    pub(crate) last_tool_name: Option<String>,
    /// A one-line status.
    #[serde(default)]
    pub(crate) summary: Option<String>,
}

/// Fields of a task that changed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskUpdated {
    pub(crate) task_id: String,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    /// What changed.
    #[serde(default)]
    pub(crate) patch: TaskPatch,
}

/// The changed fields of a task.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct TaskPatch {
    /// `pending`, `running`, `completed`, `failed`, `killed` or `paused`.
    pub(crate) status: Option<String>,
    pub(crate) description: Option<String>,
    /// When it ended, in milliseconds since 1970.
    pub(crate) end_time: Option<f64>,
    pub(crate) error: Option<String>,
    /// Whether it runs in the background.
    pub(crate) is_backgrounded: Option<bool>,
}

/// A task ended.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskNotification {
    pub(crate) task_id: String,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    /// The tool call that started it.
    #[serde(default)]
    pub(crate) tool_use_id: Option<String>,
    /// `completed`, `failed` or `stopped`.
    #[serde(default)]
    pub(crate) status: String,
    /// The file its output went to.
    #[serde(default)]
    pub(crate) output_file: Option<String>,
    /// A summary of what it did.
    #[serde(default)]
    pub(crate) summary: Option<String>,
    /// Its final counts.
    #[serde(default)]
    pub(crate) usage: Option<TaskUsage>,
}

/// A live background task.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BackgroundTask {
    pub(crate) task_id: String,
    #[serde(default)]
    pub(crate) run_id: Option<String>,
    #[serde(default)]
    pub(crate) task_type: String,
    #[serde(default)]
    pub(crate) description: String,
    /// The subagent task that started it.
    #[serde(default)]
    pub(crate) parent_task_id: Option<String>,
    /// Whether it is housekeeping.
    #[serde(default)]
    pub(crate) ambient: Option<bool>,
}

/// A hook run, from any of its three frames.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Hook {
    /// The run's id.
    pub(crate) hook_id: String,
    #[serde(default)]
    pub(crate) hook_name: String,
    /// The event it runs for.
    #[serde(default)]
    pub(crate) hook_event: String,
    /// Its combined output.
    #[serde(default)]
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) stderr: String,
    #[serde(default)]
    pub(crate) exit_code: Option<i32>,
    /// `success`, `error` or `cancelled` once it ended.
    #[serde(default)]
    pub(crate) outcome: Option<String>,
    /// Whether this frame reports output of a hook still running.
    #[serde(skip)]
    pub(crate) running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Message, parse};

    #[test]
    fn hook_frames_tell_running_from_finished() {
        let started =
            parse(r#"{"type":"system","subtype":"hook_started","hook_id":"h","hook_name":"n","hook_event":"e"}"#);
        let Message::System(System::Hook(hook)) = started.unwrap() else {
            panic!()
        };
        assert_eq!((hook.outcome, hook.running), (None, false));
        let done =
            parse(r#"{"type":"system","subtype":"hook_response","hook_id":"h","outcome":"success","exit_code":0}"#);
        let Message::System(System::Hook(hook)) = done.unwrap() else {
            panic!()
        };
        assert_eq!(hook.outcome.as_deref(), Some("success"));
    }
}
