//! The frames the CLI writes on stdout, one JSON object per line, as `@anthropic-ai/claude-agent-sdk` types them.
//!
//! Only the fields the app uses are typed. A frame whose type or subtype is unknown becomes `Message::Unknown`, one
//! of a known type whose fields do not fit becomes `Message::Malformed`, both with the raw JSON; the CLI adds types
//! and fields over time and readers are to ignore what they do not know, while a known frame that does not fit is
//! worth a diagnostic.

mod system;

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The Messages API's stream events and token counts.
pub use crate::anthropic::{BlockDelta, ContentBlock, Event, Usage as ApiUsage};
pub(crate) use system::TaskUsage;
pub use system::{
    ApiRetry, BackgroundTask, CompactBoundary, Hook, Init, System, TaskNotification, TaskProgress, TaskStarted,
    TaskUpdated,
};

/// One frame from the CLI.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// A complete content block of an assistant message.
    Assistant(Assistant),
    /// A user-role message the CLI adds, chiefly tool results.
    User(User),
    /// The end of a turn.
    Result(TurnResult),
    /// A `system` frame.
    System(System),
    /// A streaming event of the message being generated.
    StreamEvent(StreamEvent),
    /// A tool is still running.
    ToolProgress(ToolProgress),
    /// A one-line summary of a group of tool calls.
    ToolUseSummary(ToolUseSummary),
    /// The plan's rate-limit state changed.
    RateLimit(RateLimitInfo),
    /// The state of a sign-in the CLI performs.
    AuthStatus(AuthStatus),
    /// The conversation was discarded, as by `/clear`.
    ConversationReset(ConversationReset),
    /// A predicted next prompt.
    PromptSuggestion(String),
    /// A heartbeat.
    KeepAlive,
    /// A control request from the CLI; `control::Incoming` reads its body.
    ControlRequest {
        /// The id the answer echoes.
        request_id: String,
        /// The request body with its `subtype`.
        request: Value,
    },
    /// The answer to one of the app's control requests.
    ControlResponse(ControlResponse),
    /// The CLI no longer needs the answer to one of its requests.
    ControlCancel {
        /// The request withdrawn.
        request_id: String,
    },
    /// A frame of a type the app does not know, kept as it came.
    Unknown(Value),
    /// A frame of a type the app knows whose fields do not fit, kept as it came.
    Malformed(Value),
}

/// A line that is not a JSON object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotJson(pub String);

/// Reads one line of the CLI's output.
///
/// ```
/// use term_claude::protocol::{self, Block, Message};
///
/// let line = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Hi"}]}}"#;
/// let Ok(Message::Assistant(frame)) = protocol::parse(line) else { panic!("not an assistant frame") };
/// assert_eq!(frame.message.content, [Block::Text { text: "Hi".into() }]);
/// assert!(matches!(protocol::parse(r#"{"type":"from_a_later_cli"}"#), Ok(Message::Unknown(_))));
/// assert!(protocol::parse("[1, 2]").is_err());
/// ```
pub fn parse(line: &str) -> Result<Message, NotJson> {
    let value: Value = serde_json::from_str(line).map_err(|e| NotJson(e.to_string()))?;
    if !value.is_object() {
        return Err(NotJson("not an object".into()));
    }
    Ok(Message::from_value(value))
}

impl Message {
    /// The frame `value` holds.
    pub(crate) fn from_value(value: Value) -> Message {
        let kind = value.get("type").and_then(Value::as_str).unwrap_or_default();
        let parsed = match kind {
            "assistant" => typed(&value).map(Message::Assistant),
            "user" => typed(&value).map(Message::User),
            "result" => typed(&value).map(Message::Result),
            "system" => System::read(&value).map(Message::System),
            "stream_event" => typed(&value).map(Message::StreamEvent),
            "tool_progress" => typed(&value).map(Message::ToolProgress),
            "tool_use_summary" => typed(&value).map(Message::ToolUseSummary),
            "rate_limit_event" => value.get("rate_limit_info").and_then(typed).map(Message::RateLimit),
            "auth_status" => typed(&value).map(Message::AuthStatus),
            "conversation_reset" => typed(&value).map(Message::ConversationReset),
            "prompt_suggestion" => text(&value, "suggestion").map(Message::PromptSuggestion),
            "keep_alive" => Some(Message::KeepAlive),
            "control_request" => text(&value, "request_id").and_then(|request_id| {
                let request = value.get("request").filter(|r| r.is_object())?.clone();
                Some(Message::ControlRequest { request_id, request })
            }),
            "control_response" => value.get("response").and_then(typed).map(Message::ControlResponse),
            "control_cancel_request" => {
                text(&value, "request_id").map(|request_id| Message::ControlCancel { request_id })
            }
            _ => return Message::Unknown(value),
        };
        parsed.unwrap_or(Message::Malformed(value))
    }
}

pub(crate) fn typed<T: DeserializeOwned>(value: &Value) -> Option<T> {
    T::deserialize(value).ok()
}

fn text(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

/// A message of the Messages API: id, model, content blocks and usage.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct ApiMessage {
    /// Its id; consecutive assistant frames of one streamed response share it.
    pub(crate) id: String,
    /// The model that wrote it, `<synthetic>` for messages the CLI made up itself.
    pub(crate) model: Option<String>,
    /// Its content blocks; a streamed assistant frame carries just one.
    pub content: Vec<Block>,
    /// Why the response stopped; null on streamed frames.
    pub(crate) stop_reason: Option<String>,
    /// Token counts; not final on streamed frames.
    pub(crate) usage: Option<ApiUsage>,
}

/// A content block.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Text.
    Text {
        /// The text.
        #[serde(default)]
        text: String,
    },
    /// Reasoning; empty when the CLI omits it.
    Thinking {
        /// The reasoning or its summary.
        #[serde(default)]
        thinking: String,
    },
    /// Reasoning the API keeps encrypted.
    RedactedThinking,
    /// A tool call.
    ToolUse {
        /// The call's id.
        #[serde(default)]
        id: String,
        /// The tool.
        #[serde(default)]
        name: String,
        /// Its input.
        #[serde(default)]
        input: Value,
    },
    /// A tool the API runs itself, such as web search.
    ServerToolUse {
        /// The call's id.
        #[serde(default)]
        id: String,
        /// The tool.
        #[serde(default)]
        name: String,
        /// Its input.
        #[serde(default)]
        input: Value,
    },
    /// The result of a tool call.
    ToolResult {
        /// The call it answers.
        #[serde(default)]
        tool_use_id: String,
        /// A string or a list of blocks.
        #[serde(default)]
        content: Value,
        /// Whether the call failed.
        #[serde(default)]
        is_error: Option<bool>,
    },
    /// An image.
    Image {
        /// Its bytes, when the frame carries them.
        #[serde(default)]
        source: Option<ImageSource>,
    },
    /// Any other block.
    #[serde(other)]
    Other,
}

/// Where an image's bytes are.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ImageSource {
    /// `base64` when `data` holds the bytes.
    #[serde(default, rename = "type")]
    pub(crate) kind: String,
    /// Its type, such as `image/png`.
    #[serde(default)]
    pub(crate) media_type: String,
    /// The bytes in base64.
    #[serde(default)]
    pub(crate) data: String,
}

/// An assistant frame.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Assistant {
    /// The message, with the one block this frame delivers.
    pub message: ApiMessage,
    /// The tool call that started the subagent which wrote it.
    #[serde(default)]
    pub(crate) parent_tool_use_id: Option<String>,
    /// Why the request failed, for messages the CLI made up to report a failure.
    #[serde(default)]
    pub(crate) error: Option<String>,
    /// The frame's id.
    #[serde(default)]
    pub(crate) uuid: Option<String>,
    /// Whether an interrupt cut the message short.
    #[serde(default)]
    pub(crate) aborted: Option<bool>,
    /// Frames this one replaces.
    #[serde(default)]
    pub(crate) supersedes: Vec<String>,
}

/// A user-role frame from the CLI.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct User {
    pub(crate) message: UserBody,
    /// The tool call that started the subagent it belongs to.
    #[serde(default)]
    pub(crate) parent_tool_use_id: Option<String>,
    /// Whether the CLI made it up rather than the user typing it.
    #[serde(default, rename = "isSynthetic")]
    pub(crate) is_synthetic: Option<bool>,
    /// Whether it echoes a prompt the CLI took in, from the app or from Remote Control.
    #[serde(default, rename = "isReplay")]
    pub(crate) is_replay: Option<bool>,
    /// Where an echoed prompt came from, such as `{"kind": "task-notification"}`; none for a person's prompt.
    #[serde(default)]
    pub(crate) origin: Option<Value>,
    /// The tool's structured output, shaped per tool.
    #[serde(default, alias = "toolUseResult")]
    pub(crate) tool_use_result: Option<Value>,
    /// The frame's id.
    #[serde(default)]
    pub(crate) uuid: Option<String>,
}

/// The body of a user-role message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct UserBody {
    /// A string or a list of blocks.
    #[serde(default)]
    pub(crate) content: Value,
}

impl UserBody {
    /// The blocks, with a string read as one text block.
    pub(crate) fn blocks(&self) -> Vec<Block> {
        match &self.content {
            Value::String(text) => vec![Block::Text { text: text.clone() }],
            Value::Array(items) => items.iter().map(|item| typed(item).unwrap_or(Block::Other)).collect(),
            _ => Vec::new(),
        }
    }
}

/// The `result` frame at the end of a turn.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TurnResult {
    /// `success` or the sort of error.
    pub(crate) subtype: String,
    /// Whether the turn failed.
    #[serde(default)]
    pub(crate) is_error: bool,
    /// The final answer's text, or the error.
    #[serde(default)]
    pub(crate) result: Option<String>,
    /// How long the turn took.
    #[serde(default)]
    pub(crate) duration_ms: Option<u64>,
    /// How many model turns it took.
    #[serde(default)]
    pub(crate) num_turns: Option<u64>,
    /// The session's cost so far, in US dollars.
    #[serde(default)]
    pub(crate) total_cost_usd: Option<f64>,
    /// Token counts of the main loop for this turn.
    #[serde(default)]
    pub(crate) usage: Option<ApiUsage>,
    /// Totals per model for the session so far.
    #[serde(default, rename = "modelUsage")]
    pub(crate) model_usage: HashMap<String, ModelUsage>,
    /// Errors of a failed turn.
    #[serde(default)]
    pub(crate) errors: Option<Vec<String>>,
    /// Why the turn ended.
    #[serde(default)]
    pub(crate) terminal_reason: Option<String>,
    /// Why the CLI could not start, when it could not.
    #[serde(default)]
    pub(crate) startup_failure_reason: Option<String>,
    #[serde(default)]
    pub(crate) session_id: Option<String>,
}

/// Totals for one model.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelUsage {
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    /// Input tokens read from the cache.
    pub(crate) cache_read_input_tokens: u64,
    /// Input tokens written to the cache.
    pub(crate) cache_creation_input_tokens: u64,
    /// Cost in US dollars.
    #[serde(rename = "costUSD")]
    pub(crate) cost_usd: f64,
    pub(crate) context_window: Option<u64>,
}

/// A streaming event, sent with `--include-partial-messages`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamEvent {
    /// The Messages API event.
    pub(crate) event: Event,
    /// The tool call that started the subagent it belongs to.
    #[serde(default)]
    pub(crate) parent_tool_use_id: Option<String>,
}

/// A tool is still running.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolProgress {
    pub(crate) tool_use_id: String,
    #[serde(default)]
    pub(crate) tool_name: String,
    /// The tool call that started the subagent it belongs to.
    #[serde(default)]
    pub(crate) parent_tool_use_id: Option<String>,
    /// How long it has run.
    #[serde(default)]
    pub(crate) elapsed_time_seconds: f64,
}

/// A one-line summary of a group of tool calls.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolUseSummary {
    pub(crate) summary: String,
    /// The calls it sums up.
    #[serde(default)]
    pub(crate) preceding_tool_use_ids: Vec<String>,
}

/// The plan's rate-limit state.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitInfo {
    /// `allowed`, `allowed_warning` or `rejected`.
    pub(crate) status: String,
    /// When the window resets, in seconds since 1970.
    #[serde(default)]
    pub(crate) resets_at: Option<f64>,
    /// Which window, such as `five_hour`.
    #[serde(default)]
    pub(crate) rate_limit_type: Option<String>,
    /// The share used, from 0 to 1.
    #[serde(default)]
    pub(crate) utilization: Option<f64>,
}

/// The state of a sign-in the CLI performs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    /// Whether it is still going on.
    #[serde(default)]
    pub(crate) is_authenticating: bool,
    /// What it printed.
    #[serde(default)]
    pub(crate) output: Vec<String>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// The conversation was discarded.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ConversationReset {
    #[serde(default)]
    pub(crate) new_conversation_id: Option<String>,
}

/// The answer to a control request the app sent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ControlResponse {
    /// Whether the request succeeded.
    pub(crate) subtype: Outcome,
    /// The request answered.
    pub(crate) request_id: String,
    /// The success payload.
    #[serde(default)]
    pub(crate) response: Option<Value>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

/// How the CLI answered a control request, by the response's `subtype`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    /// It did what was asked.
    Success,
    /// It refused or failed.
    Error,
    /// A subtype the app does not know, which counts as a failure.
    #[serde(untagged)]
    Other(String),
}

impl Outcome {
    /// The subtype as the CLI wrote it.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Outcome::Success => "success",
            Outcome::Error => "error",
            Outcome::Other(other) => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_line_that_is_not_a_json_object_is_refused() {
        assert!(parse("not json").is_err());
        assert!(parse("[1, 2]").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn unknown_types_and_subtypes_are_kept_raw() {
        let Message::Unknown(raw) = parse(r#"{"type":"warp","x":1}"#).unwrap() else {
            panic!()
        };
        assert_eq!(raw["x"], 1);
        let message = parse(r#"{"type":"system","subtype":"brand_new","session_id":"s"}"#).unwrap();
        assert_eq!(message, Message::System(System::Unknown("brand_new".into())));
    }

    #[test]
    fn a_known_type_that_does_not_fit_is_kept_raw_instead_of_failing() {
        let line = r#"{"type":"system","subtype":"task_progress","task_id":7}"#;
        assert!(matches!(parse(line).unwrap(), Message::Malformed(_)));
        let line = r#"{"type":"control_request","request_id":"r","request":"text"}"#;
        assert!(matches!(parse(line).unwrap(), Message::Malformed(_)));
    }

    #[test]
    fn extra_fields_and_unknown_blocks_do_not_stop_an_assistant_frame() {
        let frame = json!({
            "type": "assistant", "parent_tool_use_id": null, "new_field": {"a": 1},
            "message": {"id": "m", "content": [
                {"type": "redacted_thinking", "data": "xyz"},
                {"type": "hologram"},
                {"type": "text", "text": "hi", "citations": []}
            ]}
        });
        let Message::Assistant(a) = Message::from_value(frame) else {
            panic!()
        };
        assert_eq!(
            a.message.content,
            [Block::RedactedThinking, Block::Other, Block::Text { text: "hi".into() }]
        );
    }

    #[test]
    fn user_content_may_be_a_string_or_blocks() {
        let text = UserBody {
            content: json!("plain"),
        };
        assert_eq!(text.blocks(), [Block::Text { text: "plain".into() }]);
        let blocks = UserBody {
            content: json!([{"type": "tool_result", "tool_use_id": "t", "content": "ok"}, {"type": "new_kind"}]),
        };
        assert!(matches!(&blocks.blocks()[0], Block::ToolResult { tool_use_id, .. } if tool_use_id == "t"));
        assert_eq!(blocks.blocks()[1], Block::Other);
    }

    #[test]
    fn control_response_outcomes_read_from_their_subtype() {
        for (subtype, outcome) in [
            ("success", Outcome::Success),
            ("error", Outcome::Error),
            ("cancelled", Outcome::Other("cancelled".into())),
        ] {
            let read: Outcome = serde_json::from_value(json!(subtype)).unwrap();
            assert_eq!(read, outcome);
            assert_eq!(read.as_str(), subtype);
        }
    }

    #[test]
    fn control_frames_are_told_apart() {
        let request = parse(r#"{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool"}}"#);
        assert!(matches!(request.unwrap(), Message::ControlRequest { request_id, .. } if request_id == "r1"));
        let response =
            parse(r#"{"type":"control_response","response":{"subtype":"error","request_id":"r2","error":"no"}}"#);
        let Message::ControlResponse(r) = response.unwrap() else {
            panic!()
        };
        assert_eq!((r.subtype, r.error.as_deref()), (Outcome::Error, Some("no")));
        let cancel = parse(r#"{"type":"control_cancel_request","request_id":"r3"}"#).unwrap();
        assert_eq!(
            cancel,
            Message::ControlCancel {
                request_id: "r3".into()
            }
        );
        assert_eq!(parse(r#"{"type":"keep_alive"}"#).unwrap(), Message::KeepAlive);
    }
}
