//! The partial frames the CLI streams while the agent writes.

use std::time::SystemTime;

use term_thread::{AgentMessage, EntryKind, Lane, Thought, Update};

use super::context::context_used;
use super::{Mapper, Slot, Stream, Streamed, lane, put};
use crate::protocol::{ApiUsage, BlockDelta, ContentBlock, Event, StreamEvent};

impl Mapper {
    pub(super) fn stream(&mut self, e: &StreamEvent, now: SystemTime) -> Vec<Update> {
        let lane = lane(e.parent_tool_use_id.as_ref());
        match &e.event {
            Event::MessageStart { message } => {
                let input = message.usage.as_ref().map_or(0, ApiUsage::context_input);
                self.streams.insert(
                    lane.clone(),
                    Stream {
                        message: message.id.clone(),
                        input_tokens: input,
                    },
                );
                if lane == Lane::Main && input > 0 {
                    return vec![context_used(input)];
                }
                Vec::new()
            }
            Event::ContentBlockStart { index, content_block } => self.block_start(&lane, *index, content_block, now),
            Event::ContentBlockDelta { index, delta } => {
                let text = match delta {
                    BlockDelta::TextDelta { text } => text,
                    BlockDelta::ThinkingDelta { thinking } => thinking,
                    _ => return Vec::new(),
                };
                let Some(stream) = self.streams.get(&lane) else {
                    return Vec::new();
                };
                vec![Update::Append {
                    key: format!("{}:{index}", stream.message),
                    text: text.clone(),
                }]
            }
            Event::ContentBlockStop { index } => {
                let Some(stream) = self.streams.get(&lane) else {
                    return Vec::new();
                };
                let key = format!("{}:{index}", stream.message);
                if !self.open.contains(&key) {
                    return Vec::new();
                }
                self.open.retain(|k| k != &key);
                vec![Update::Finish {
                    key,
                    text: None,
                    at: now,
                }]
            }
            Event::MessageDelta { usage: Some(usage), .. } if lane == Lane::Main => {
                let input = self.streams.get(&lane).map_or(0, |s| s.input_tokens);
                vec![context_used(input + usage.output_tokens.unwrap_or(0))]
            }
            _ => Vec::new(),
        }
    }

    fn block_start(&mut self, lane: &Lane, index: usize, block: &ContentBlock, now: SystemTime) -> Vec<Update> {
        let Some(message) = self.streams.get(lane).map(|s| s.message.clone()) else {
            return Vec::new();
        };
        let key = format!("{message}:{index}");
        let (slot, entry) = match block {
            ContentBlock::Text { .. } => (
                Slot::Text,
                EntryKind::AgentMessage(AgentMessage {
                    markdown: String::new(),
                    streaming: true,
                }),
            ),
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => (
                Slot::Thought,
                EntryKind::Thought(Thought {
                    text: String::new(),
                    duration: None,
                    streaming: true,
                }),
            ),
            ContentBlock::ToolUse { id, name, input } | ContentBlock::ServerToolUse { id, name, input } => {
                let parent = match lane {
                    Lane::Main => None,
                    Lane::Subagent(p) => Some(p.clone()),
                };
                let mut out = Vec::new();
                if !self.tools.contains_key(id) {
                    self.tool_use(lane, parent.as_ref(), id, name, input, false, now, &mut out);
                }
                return out;
            }
            _ => return Vec::new(),
        };
        self.streamed.entry(message).or_default().push(Streamed {
            slot,
            key: key.clone(),
            claimed: false,
        });
        self.open.push(key.clone());
        vec![put(lane, Some(key), now, entry)]
    }
}
