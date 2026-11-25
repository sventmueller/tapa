//! The events of the Messages API's stream, as the Claude Code CLI passes them on with `--include-partial-messages`.
//! A missing or null field reads as empty, and an event or block of a type not listed here as `Other`, so a server
//! that leaves something out costs that part only.
//!
//! ```
//! use term_claude::anthropic::{BlockDelta, Event};
//!
//! let event: Event = serde_json::from_str(
//!     r#"{"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Hi"}}"#,
//! )?;
//! let Event::ContentBlockDelta { index: 1, delta: BlockDelta::TextDelta { text } } = event else { panic!() };
//! assert_eq!(text, "Hi");
//! # Ok::<(), serde_json::Error>(())
//! ```

use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// One event of the stream.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A message begins.
    MessageStart {
        /// The message so far.
        #[serde(default, deserialize_with = "or_default")]
        message: Started,
    },
    /// A content block begins.
    ContentBlockStart {
        /// Its position in the message.
        #[serde(default, deserialize_with = "or_default")]
        index: usize,
        /// The block so far.
        #[serde(default, deserialize_with = "or_default")]
        content_block: ContentBlock,
    },
    /// More of a content block.
    ContentBlockDelta {
        /// Its position in the message.
        #[serde(default, deserialize_with = "or_default")]
        index: usize,
        /// What arrived.
        #[serde(default, deserialize_with = "or_default")]
        delta: BlockDelta,
    },
    /// A content block ends.
    ContentBlockStop {
        /// Its position in the message.
        #[serde(default, deserialize_with = "or_default")]
        index: usize,
    },
    /// Why the message stopped, and its token counts.
    MessageDelta {
        /// Why it stopped.
        #[serde(default, deserialize_with = "or_default")]
        delta: MessageDelta,
        /// Its token counts so far.
        #[serde(default)]
        usage: Option<Usage>,
    },
    /// The message ends.
    MessageStop,
    /// The stream fails, such as with `overloaded_error`.
    Error {
        /// The error object: its `type` and `message`.
        #[serde(default)]
        error: Value,
    },
    /// Any other event, such as `ping`.
    #[serde(other)]
    Other,
}

/// The message as `message_start` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Started {
    #[serde(default, deserialize_with = "or_default")]
    pub id: String,
    /// The model that writes it.
    #[serde(default)]
    pub model: Option<String>,
    /// Its token counts so far.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Token counts of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Usage {
    /// Input tokens not read from or written to the cache.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Input tokens written to the cache.
    pub cache_creation_input_tokens: Option<u64>,
    /// Input tokens read from the cache.
    pub cache_read_input_tokens: Option<u64>,
}

impl Usage {
    /// All input tokens, cached or not: what the context window holds before the output.
    pub fn context_input(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
    }
}

/// A content block as `content_block_start` begins it.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text.
    Text {
        /// The text so far.
        #[serde(default, deserialize_with = "or_default")]
        text: String,
    },
    /// Reasoning.
    Thinking {
        /// The reasoning so far.
        #[serde(default, deserialize_with = "or_default")]
        thinking: String,
        /// What the API checks the reasoning by when it is sent back.
        #[serde(default)]
        signature: Option<String>,
    },
    /// Reasoning the API keeps encrypted.
    RedactedThinking {
        /// The encrypted reasoning, sent back as it is.
        #[serde(default, deserialize_with = "or_default")]
        data: String,
    },
    /// A tool call; its input follows in `input_json_delta`s.
    ToolUse {
        /// The call's id.
        #[serde(default, deserialize_with = "or_default")]
        id: String,
        /// The tool.
        #[serde(default, deserialize_with = "or_default")]
        name: String,
        /// Its input so far, usually empty.
        #[serde(default)]
        input: Value,
    },
    /// A tool the API runs itself, such as web search.
    ServerToolUse {
        /// The call's id.
        #[serde(default, deserialize_with = "or_default")]
        id: String,
        /// The tool.
        #[serde(default, deserialize_with = "or_default")]
        name: String,
        /// Its input so far.
        #[serde(default)]
        input: Value,
    },
    /// The model was switched for the server-side refusal fallback; what came before it is void.
    Fallback {
        /// The model that refused.
        #[serde(default, deserialize_with = "or_default")]
        from: FallbackModel,
        /// The model that answers instead.
        #[serde(default, deserialize_with = "or_default")]
        to: FallbackModel,
    },
    /// Any other block.
    #[default]
    #[serde(other)]
    Other,
}

/// One side of a fallback.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct FallbackModel {
    /// The model's id.
    #[serde(default, deserialize_with = "or_default")]
    pub model: String,
}

/// What `content_block_delta` adds to a block.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockDelta {
    /// Text.
    TextDelta {
        /// The text.
        #[serde(default, deserialize_with = "or_default")]
        text: String,
    },
    /// Reasoning.
    ThinkingDelta {
        /// The reasoning.
        #[serde(default, deserialize_with = "or_default")]
        thinking: String,
    },
    /// The reasoning's signature.
    SignatureDelta {
        /// The signature.
        #[serde(default, deserialize_with = "or_default")]
        signature: String,
    },
    /// Part of a tool call's input as JSON text.
    InputJsonDelta {
        /// The JSON text.
        #[serde(default, deserialize_with = "or_default")]
        partial_json: String,
    },
    /// Anything else.
    #[default]
    #[serde(other)]
    Other,
}

/// What `message_delta` says about the end of the message.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct MessageDelta {
    /// Why it stopped, such as `end_turn` or `refusal`.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// More about a refusal.
    #[serde(default)]
    pub stop_details: Option<StopDetails>,
}

/// More about why a message stopped.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct StopDetails {
    /// The kind of refusal, such as `cyber`.
    #[serde(default)]
    pub category: Option<String>,
}

/// A field's value, or its default where it is null.
fn or_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(deserializer: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: &str) -> Event {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn every_event_of_a_stream_reads_into_its_type() {
        let Event::MessageStart { message } = event(
            r#"{"type":"message_start","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":7,"cache_read_input_tokens":3}}}"#,
        ) else {
            panic!()
        };
        assert_eq!(
            (message.id.as_str(), message.model.as_deref()),
            ("m1", Some("claude-opus-5-5"))
        );
        assert_eq!(message.usage.unwrap().context_input(), 10);
        assert_eq!(
            event(
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t","name":"grep","input":{}}}"#
            ),
            Event::ContentBlockStart {
                index: 2,
                content_block: ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "grep".into(),
                    input: serde_json::json!({})
                }
            }
        );
        assert_eq!(
            event(r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"s"}}"#),
            Event::ContentBlockDelta {
                index: 0,
                delta: BlockDelta::SignatureDelta { signature: "s".into() }
            }
        );
        let Event::MessageDelta { delta, usage } = event(
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"category":"cyber"}},"usage":{"output_tokens":5}}"#,
        ) else {
            panic!()
        };
        assert_eq!(delta.stop_reason.as_deref(), Some("refusal"));
        assert_eq!(delta.stop_details.and_then(|d| d.category).as_deref(), Some("cyber"));
        assert_eq!(usage.and_then(|u| u.output_tokens), Some(5));
        assert_eq!(event(r#"{"type":"message_stop"}"#), Event::MessageStop);
        assert!(matches!(
            event(r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#),
            Event::Error { error } if error["type"] == "overloaded_error"
        ));
    }

    #[test]
    fn unknown_types_missing_and_null_fields_read_as_empty() {
        assert_eq!(event(r#"{"type":"ping"}"#), Event::Other);
        assert_eq!(
            event(r#"{"type":"content_block_start","content_block":{"type":"web_search_tool_result"}}"#),
            Event::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Other
            }
        );
        assert_eq!(
            event(r#"{"type":"content_block_start","index":null,"content_block":{"type":"text","text":null}}"#),
            Event::ContentBlockStart {
                index: 0,
                content_block: ContentBlock::Text { text: String::new() }
            }
        );
        assert_eq!(
            event(
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"fallback","from":{"model":"a"}}}"#
            ),
            Event::ContentBlockStart {
                index: 1,
                content_block: ContentBlock::Fallback {
                    from: FallbackModel { model: "a".into() },
                    to: FallbackModel::default()
                }
            }
        );
        assert!(
            matches!(event(r#"{"type":"message_start"}"#), Event::MessageStart { message } if message.id.is_empty())
        );
        assert!(serde_json::from_str::<Event>(r#"{"type":"content_block_stop","index":"one"}"#).is_err());
    }
}
