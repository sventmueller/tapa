//! How the server's events change the thread (E147); the crate documentation has the table this follows. Text and
//! reasoning are keyed by their assistant message and ordinal, tools by their message and call id, so a session
//! continuing a thread another one filled never overwrites its entries; what the thread model has no place for is
//! ignored, never guessed at.

mod cards;
mod history;
mod usage;

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use term_thread::{
    Activity, AgentMessage, EntryKind, ErrorKind, InfoLevel, Lane, Notice, Status, Thought, ToolCall, ToolOutput,
    ToolPatch, ToolStatus, Update, UserMessage,
};

use crate::capabilities::{Capabilities, ModelRef};

pub(crate) use usage::rate_limits;

/// How the notice of a turn the server stopped for another reason than the user, such as its shutdown, begins; the
/// reason follows.
pub const OTHER_INTERRUPTION: &str = "interrupted: ";

/// The most command output a card keeps while it streams; older output is dropped from the front.
const OUTPUT_LIMIT: usize = 512 * 1024;

/// A tool call the mapper has seen begin.
#[derive(Debug, Clone, PartialEq)]
struct Tool {
    key: String,
    name: String,
    input: Value,
}

/// The last `OUTPUT_LIMIT` bytes of `output`, starting at a whole character.
fn output_tail(output: &str) -> &str {
    let Some(mut cut) = output.len().checked_sub(OUTPUT_LIMIT) else {
        return output;
    };
    while !output.is_char_boundary(cut) {
        cut += 1;
    }
    &output[cut..]
}

/// Turns what the server reports into changes of the thread.
#[derive(Debug, Default)]
pub struct Mapper {
    /// This mapper's part of the keys it makes up itself.
    origin: u64,
    next: u64,
    session: Option<String>,
    running: bool,
    /// An error notice was shown for the running turn, so its failed end adds no second one.
    failed: bool,
    started: Option<SystemTime>,
    /// The keys of the text and reasoning that stream, with whether each is reasoning.
    open: HashMap<String, bool>,
    tools: HashMap<String, Tool>,
    usage: usage::Tally,
}

pub(crate) fn put(key: String, at: SystemTime, entry: EntryKind) -> Update {
    Update::Put {
        lane: Lane::Main,
        key: Some(key),
        at,
        entry,
    }
}

pub(crate) fn text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

/// A number of the schema, which types its counts as `number`, as a whole one; `None` for a negative one.
pub(crate) fn whole(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let n = value.as_f64()?.round();
        Duration::try_from_secs_f64(n).ok().map(|d| d.as_secs())
    })
}

/// The duration of a number of milliseconds.
pub(crate) fn duration_ms(value: &Value) -> Option<Duration> {
    Duration::try_from_secs_f64(value.as_f64()? / 1000.0).ok()
}

/// The time of a field in milliseconds since 1970, else `fallback`.
pub(crate) fn millis(value: &Value, name: &str, fallback: SystemTime) -> SystemTime {
    value
        .get(name)
        .and_then(duration_ms)
        .filter(|d| !d.is_zero())
        .map_or(fallback, |d| UNIX_EPOCH + d)
}

/// The kind of error notice a `Session.StructuredError` stands for.
pub(crate) fn error_kind(error: &Value) -> ErrorKind {
    let kind = text(error, "type").unwrap_or_default().to_ascii_lowercase();
    match error.get("status").and_then(Value::as_u64) {
        Some(401 | 403) => ErrorKind::Auth,
        _ if kind.contains("auth") => ErrorKind::Auth,
        _ if kind.contains("context") || kind.contains("length") => ErrorKind::Turn,
        _ if kind.starts_with("tool") || kind.contains("permission") => ErrorKind::Other,
        _ => ErrorKind::Api,
    }
}

/// The message of a `Session.StructuredError`, with the body of the provider's answer when there is one.
pub(crate) fn error_message(error: &Value) -> String {
    let message = text(error, "message").unwrap_or_default();
    let body = error
        .get("response")
        .and_then(|r| text(r, "body"))
        .filter(|b| !b.is_empty());
    match body {
        Some(body) if !message.contains(body) => format!("{message}\n{body}"),
        _ => message.to_string(),
    }
}

fn message_key(data: &Value, prefix: &str) -> Option<String> {
    let message = text(data, "assistantMessageID")?;
    let ordinal = data.get("ordinal").and_then(Value::as_u64).unwrap_or_default();
    Some(format!("{prefix}:{message}:{ordinal}"))
}

/// The name the cards give the tool, or the permission action, OpenCode calls `tool`.
pub(crate) fn claude_name(tool: &str) -> String {
    cards::card_for(tool, &Value::Null).name
}

/// The thread entry of the tool call `id` of the assistant message `message`.
/// Whether the tool `name` only sets the to-do list, which the thread shows as a list and not as a card, as it
/// does for Claude's `TodoWrite`.
fn is_list(name: &str) -> bool {
    name == "todowrite"
}

/// Names OpenCode's message `id` for the user message `key`, which `revert/stage` takes to drop it and every later
/// one.
pub(crate) fn chain(key: String, id: &str) -> Update {
    Update::Chain {
        key,
        uuid: id.to_string(),
        follows: Some(String::new()),
    }
}

/// The key of the user message among `updates`, as `begin_turn` and `steered` put it.
pub(crate) fn user_key(updates: &[Update]) -> Option<String> {
    updates.iter().find_map(|update| match update {
        Update::Put {
            key,
            entry: EntryKind::UserMessage(_),
            ..
        } => key.clone(),
        _ => None,
    })
}

pub(crate) fn tool_key(message: &str, id: &str) -> String {
    format!("tool:{message}:{id}")
}

fn streaming(key: String, at: SystemTime, thought: bool) -> Update {
    let entry = if thought {
        EntryKind::Thought(Thought {
            streaming: true,
            ..Thought::default()
        })
    } else {
        EntryKind::AgentMessage(AgentMessage {
            markdown: String::new(),
            streaming: true,
        })
    };
    put(key, at, entry)
}

impl Mapper {
    /// A mapper whose own keys carry `origin`, such as the time its session started, so that they differ from those
    /// of every other mapper that filled the same thread.
    pub fn new(origin: u64) -> Mapper {
        Mapper {
            origin,
            ..Mapper::default()
        }
    }

    /// Takes events only for `session`; those of other sessions, such as a subagent's, are left out.
    pub fn set_session(&mut self, session: &str) {
        self.session = Some(session.to_string());
    }

    /// The thread entry of the tool call `call` that has begun and not settled.
    pub(crate) fn tool_entry(&self, call: &str) -> Option<String> {
        self.tools.get(call).map(|t| t.key.clone())
    }

    /// Whether a turn runs, as far as the events said.
    pub fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn key(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{:x}-{}", self.origin, self.next)
    }

    /// Ends the text and reasoning that stream now.
    pub(crate) fn settle(&mut self, at: SystemTime) -> Vec<Update> {
        let mut keys: Vec<String> = self.open.drain().map(|(key, _)| key).collect();
        keys.sort();
        keys.into_iter()
            .map(|key| Update::Finish { key, text: None, at })
            .collect()
    }

    /// A message the user sent, which starts a turn.
    pub(crate) fn begin_turn(&mut self, message: &UserMessage, at: SystemTime) -> Vec<Update> {
        let key = self.key("user");
        self.failed = false;
        let mut out = self.settle(at);
        out.push(put(key, at, EntryKind::UserMessage(message.clone())));
        out.push(Update::Status(Status::Running));
        out
    }

    /// A message the user added to the running turn.
    pub(crate) fn steered(&mut self, message: &UserMessage, at: SystemTime) -> Vec<Update> {
        let key = self.key("user");
        vec![put(key, at, EntryKind::UserMessage(message.clone()))]
    }

    /// The turn is over without the server saying so, such as when the prompt failed or the process ended.
    pub(crate) fn abandon(&mut self, at: SystemTime) -> Vec<Update> {
        self.running = false;
        let mut out = self.settle(at);
        let open: Vec<Tool> = self.tools.drain().map(|(_, tool)| tool).collect();
        for tool in open {
            out.push(Update::Tool {
                key: tool.key,
                patch: ToolPatch {
                    status: Some(ToolStatus::Failed),
                    ended: Some(at),
                    ..ToolPatch::default()
                },
            });
        }
        out.push(Update::Activity(None));
        out.push(Update::Status(Status::Idle));
        out
    }

    /// The thread's state after its history was read again following a gap in the event stream: running when the
    /// server's `Session.Info` says the session is busy, else at rest, with the totals `info` names.
    pub(crate) fn resynced(&mut self, info: &Value, at: SystemTime) -> Vec<Update> {
        let time = info.get("time").unwrap_or(&Value::Null);
        let since = |name: &str| time.get(name).and_then(Value::as_f64);
        let busy = match (since("busy"), since("idle")) {
            (Some(busy), Some(idle)) => busy > idle,
            (Some(_), None) => true,
            _ => false,
        };
        let mut out = if busy {
            self.running = true;
            vec![Update::Status(Status::Running)]
        } else {
            self.abandon(at)
        };
        out.extend(self.usage.totals(info));
        out
    }

    /// A failure to show in the thread, such as a prompt the server refused.
    pub(crate) fn error(&mut self, kind: ErrorKind, message: String, at: SystemTime) -> Vec<Update> {
        let mut out = self.settle(at);
        let key = self.key("notice");
        out.push(put(key, at, EntryKind::Notice(Notice::Error { kind, message })));
        out
    }

    fn notice(&mut self, level: InfoLevel, text: String, at: SystemTime) -> Update {
        let key = self.key("notice");
        put(key, at, EntryKind::Notice(Notice::Info { level, text }))
    }

    /// Maps one event of the stream, a `V2Event` with its `type` and `data`; `caps` learns what changed about the
    /// session, and the flag says whether it did.
    pub fn event(&mut self, event: &Value, caps: &mut Capabilities, at: SystemTime) -> (Vec<Update>, bool) {
        let (Some(kind), Some(data)) = (text(event, "type"), event.get("data")) else {
            return (Vec::new(), false);
        };
        if let (Some(ours), Some(theirs)) = (&self.session, text(data, "sessionID"))
            && ours != theirs
        {
            return (Vec::new(), false);
        }
        let at = millis(event, "created", at);
        match kind {
            "session.agent.selected" => return self.agent_selected(data, caps),
            "session.model.selected" => return self.model_selected(data, caps),
            _ => {}
        }
        let updates = match kind {
            "session.execution.started" => self.execution_started(at),
            "session.execution.succeeded" => self.execution_ended(None, at),
            "session.idle" if self.running => self.execution_ended(None, at),
            "session.execution.failed" => self.execution_failed(data, at),
            "session.execution.interrupted" => {
                let reason = text(data, "reason").unwrap_or("user");
                self.execution_ended(Some(reason), at)
            }
            "session.step.started" => vec![Update::Activity(Some(Activity::Requesting))],
            "session.step.streamed" => vec![Update::Activity(None)],
            "session.step.ended" => {
                let mut out = vec![Update::Activity(None)];
                out.extend(self.usage.step(data, caps.context_size()));
                out
            }
            "session.step.failed" => self.step_failed(data, caps, at),
            "session.text.started" => self.started(data, at, false),
            "session.reasoning.started" => self.started(data, at, true),
            "session.text.delta" => self.delta(data, at, false),
            "session.reasoning.delta" => self.delta(data, at, true),
            "session.text.ended" => self.ended(data, at, false),
            "session.reasoning.ended" => self.ended(data, at, true),
            "session.tool.input.started" => self.tool_announced(data, at),
            "session.tool.called" => self.tool_called(data, at),
            "session.tool.progress" => self.tool_progress(data),
            "session.tool.success" => self.tool_settled(data, false, at),
            "session.tool.failed" => self.tool_settled(data, true, at),
            "session.retry.scheduled" => self.retry(data, at),
            "session.compaction.started" => vec![Update::Activity(Some(Activity::Compacting))],
            "session.compaction.ended" => self.compacted(data, at),
            "session.compaction.failed" => {
                let error = data.get("error").unwrap_or(&Value::Null);
                let mut out = vec![Update::Activity(None)];
                out.extend(self.error(ErrorKind::Compaction, error_message(error), at));
                out
            }
            "session.renamed" => text(data, "title")
                .filter(|t| !t.is_empty())
                .map(|title| Update::Title(title.to_string()))
                .into_iter()
                .collect(),
            "session.usage.updated" => self.usage.totals(data),
            _ => Vec::new(),
        };
        (updates, false)
    }

    fn agent_selected(&mut self, data: &Value, caps: &mut Capabilities) -> (Vec<Update>, bool) {
        let Some(agent) = text(data, "agent") else {
            return (Vec::new(), false);
        };
        caps.agent = Some(agent.to_string());
        (vec![Update::Mode(agent.to_string())], true)
    }

    fn model_selected(&mut self, data: &Value, caps: &mut Capabilities) -> (Vec<Update>, bool) {
        let Some(model) = data.get("model").and_then(ModelRef::from_json) else {
            return (Vec::new(), false);
        };
        let updates = vec![Update::Model(model.id()), Update::Effort(model.variant.clone())];
        caps.model = Some(model);
        (updates, true)
    }

    fn execution_started(&mut self, at: SystemTime) -> Vec<Update> {
        self.running = true;
        self.failed = false;
        self.started = Some(at);
        self.usage.turn_started();
        vec![Update::Status(Status::Running)]
    }

    /// The end of the turn; `interrupted` names why the server stopped it.
    fn execution_ended(&mut self, interrupted: Option<&str>, at: SystemTime) -> Vec<Update> {
        let mut out = self.settle(at);
        match interrupted {
            Some("user") => {
                let key = self.key("notice");
                out.push(put(key, at, EntryKind::Notice(Notice::Interrupted)));
            }
            Some(reason) => out.push(self.notice(InfoLevel::Notice, format!("{OTHER_INTERRUPTION}{reason}"), at)),
            None => {}
        }
        if self.running {
            let duration = self.started.take().and_then(|s| at.duration_since(s).ok());
            out.extend(self.usage.turn_completed(duration));
        }
        let open: Vec<Tool> = self.tools.drain().map(|(_, tool)| tool).collect();
        for tool in open {
            out.push(Update::Tool {
                key: tool.key,
                patch: ToolPatch {
                    status: Some(if interrupted.is_some() {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Completed
                    }),
                    ended: Some(at),
                    ..ToolPatch::default()
                },
            });
        }
        self.running = false;
        self.failed = false;
        out.push(Update::Activity(None));
        out.push(Update::Status(Status::Idle));
        out
    }

    fn execution_failed(&mut self, data: &Value, at: SystemTime) -> Vec<Update> {
        let mut out = Vec::new();
        if !self.failed {
            let error = data.get("error").unwrap_or(&Value::Null);
            let message = Some(error_message(error))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "failed".into());
            out.extend(self.error(error_kind(error), message, at));
        }
        out.extend(self.execution_ended(None, at));
        out
    }

    fn step_failed(&mut self, data: &Value, caps: &Capabilities, at: SystemTime) -> Vec<Update> {
        let error = data.get("error").unwrap_or(&Value::Null);
        self.failed = true;
        let mut out = vec![Update::Activity(None)];
        out.extend(self.usage.step(data, caps.context_size()));
        out.extend(self.error(error_kind(error), error_message(error), at));
        out
    }

    fn started(&mut self, data: &Value, at: SystemTime, thought: bool) -> Vec<Update> {
        let prefix = if thought { "reasoning" } else { "text" };
        let Some(key) = message_key(data, prefix) else {
            return Vec::new();
        };
        self.open.insert(key.clone(), thought);
        vec![Update::Activity(None), streaming(key, at, thought)]
    }

    fn delta(&mut self, data: &Value, at: SystemTime, thought: bool) -> Vec<Update> {
        let prefix = if thought { "reasoning" } else { "text" };
        let (Some(key), Some(delta)) = (message_key(data, prefix), text(data, "delta")) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !self.open.contains_key(&key) {
            self.open.insert(key.clone(), thought);
            out.push(streaming(key.clone(), at, thought));
        }
        out.push(Update::Append {
            key,
            text: delta.to_string(),
        });
        out
    }

    fn ended(&mut self, data: &Value, at: SystemTime, thought: bool) -> Vec<Update> {
        let prefix = if thought { "reasoning" } else { "text" };
        let Some(key) = message_key(data, prefix) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if self.open.remove(&key).is_none() {
            out.push(streaming(key.clone(), at, thought));
        }
        let full = text(data, "text").map(str::to_string);
        out.push(Update::Finish { key, text: full, at });
        out
    }

    fn tool_ref(data: &Value) -> Option<(String, String)> {
        let id = text(data, "id")?;
        Some((id.to_string(), tool_key(text(data, "assistantMessageID")?, id)))
    }

    fn tool_announced(&mut self, data: &Value, at: SystemTime) -> Vec<Update> {
        let (Some((id, key)), Some(name)) = (Self::tool_ref(data), text(data, "name")) else {
            return Vec::new();
        };
        let card = cards::card_for(name, &Value::Null);
        self.tools.insert(
            id.clone(),
            Tool {
                key: key.clone(),
                name: name.to_string(),
                input: Value::Null,
            },
        );
        if is_list(name) {
            return vec![Update::Activity(None)];
        }
        let mut call = ToolCall::new(id, card.name, card.kind);
        call.title = card.title;
        call.started = Some(at);
        vec![Update::Activity(None), put(key, at, EntryKind::ToolCall(call))]
    }

    fn tool_called(&mut self, data: &Value, at: SystemTime) -> Vec<Update> {
        let Some((id, key)) = Self::tool_ref(data) else {
            return Vec::new();
        };
        let input = data.get("input").cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        if !self.tools.contains_key(&id) {
            let call = ToolCall::new(id.clone(), "tool", term_thread::ToolKind::Other);
            out.push(put(key.clone(), at, EntryKind::ToolCall(call)));
        }
        let tool = self.tools.entry(id).or_insert_with(|| Tool {
            key: key.clone(),
            name: "tool".into(),
            input: Value::Null,
        });
        tool.input = input.clone();
        let todos = (tool.name == "todowrite").then(|| cards::todos(&input)).flatten();
        if is_list(&tool.name) {
            return todos
                .map(|list| {
                    vec![
                        Update::Todos(list.clone()),
                        put(format!("todos:{key}"), at, EntryKind::Todos(list)),
                    ]
                })
                .unwrap_or_default();
        }
        let card = cards::card_for(&tool.name, &input);
        let patch = ToolPatch {
            input: Some(input),
            title: Some(card.title),
            view: Some(card.view).filter(|v| *v != term_thread::ToolView::Other),
            status: Some(ToolStatus::InProgress),
            started: Some(at),
            ..ToolPatch::default()
        };
        out.push(Update::Tool {
            key: key.clone(),
            patch,
        });
        if let Some(list) = todos {
            out.push(Update::Todos(list.clone()));
            out.push(put(format!("todos:{key}"), at, EntryKind::Todos(list)));
        }
        out
    }

    fn tool_progress(&mut self, data: &Value) -> Vec<Update> {
        let Some((id, _)) = Self::tool_ref(data) else {
            return Vec::new();
        };
        let Some(tool) = self.tools.get(&id).filter(|t| !is_list(&t.name)) else {
            return Vec::new();
        };
        let Some(output) = data.get("metadata").and_then(|m| text(m, "output")) else {
            return Vec::new();
        };
        let patch = ToolPatch {
            output: Some(ToolOutput::Terminal {
                stdout: output_tail(output).to_string(),
                stderr: String::new(),
                interrupted: false,
            }),
            ..ToolPatch::default()
        };
        vec![Update::Tool {
            key: tool.key.clone(),
            patch,
        }]
    }

    fn tool_settled(&mut self, data: &Value, failed: bool, at: SystemTime) -> Vec<Update> {
        let Some((id, key)) = Self::tool_ref(data) else {
            return Vec::new();
        };
        let tool = self.tools.remove(&id);
        if tool.as_ref().is_some_and(|t| is_list(&t.name)) {
            return Vec::new();
        }
        let (name, input) = tool.map_or(("tool".to_string(), Value::Null), |t| (t.name, t.input));
        let output = if failed {
            let error = data.get("error").unwrap_or(&Value::Null);
            let message = cards::content_text(data.get("content")).unwrap_or_else(|| error_message(error));
            Some(ToolOutput::Text(message))
        } else {
            cards::output(&name, &input, data.get("content"), data.get("metadata"))
        };
        let patch = ToolPatch {
            status: Some(if failed {
                ToolStatus::Failed
            } else {
                ToolStatus::Completed
            }),
            output,
            ended: Some(at),
            ..ToolPatch::default()
        };
        vec![Update::Tool { key, patch }]
    }

    fn retry(&mut self, data: &Value, at: SystemTime) -> Vec<Update> {
        let error = data.get("error").unwrap_or(&Value::Null);
        let attempt = data.get("attempt").and_then(Value::as_u64).unwrap_or(1);
        let key = format!("retry:{}", text(data, "assistantMessageID").unwrap_or_default());
        let notice = Notice::Info {
            level: InfoLevel::Warning,
            text: format!("{} ({attempt})", error_message(error)),
        };
        vec![put(key, at, EntryKind::Notice(notice))]
    }

    fn compacted(&mut self, data: &Value, at: SystemTime) -> Vec<Update> {
        let notice = Notice::Compaction {
            manual: text(data, "reason") == Some("manual"),
            tokens_before: self.usage.context_used().unwrap_or_default(),
            tokens_after: None,
        };
        let key = self.key("compaction");
        vec![Update::Activity(None), put(key, at, EntryKind::Notice(notice))]
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn streamed_command_output_keeps_its_last_part_from_a_whole_character() {
        assert_eq!(output_tail("short"), "short");
        let long = format!("{}ä{}", "x".repeat(10), "y".repeat(OUTPUT_LIMIT - 1));
        assert_eq!(output_tail(&long), "y".repeat(OUTPUT_LIMIT - 1));
        let mut mapper = Mapper::new(1);
        let mut caps = Capabilities::default();
        let event = |kind: &str, data: Value| json!({"type": kind, "data": data});
        let tool = json!({"id": "call_1", "assistantMessageID": "msg_1", "name": "shell"});
        mapper.event(&event("session.tool.input.started", tool), &mut caps, UNIX_EPOCH);
        let progress = json!({"id": "call_1", "assistantMessageID": "msg_1", "metadata": {"output": long}});
        let (updates, _) = mapper.event(&event("session.tool.progress", progress), &mut caps, UNIX_EPOCH);
        let [Update::Tool { key, patch }] = updates.as_slice() else {
            panic!("{updates:?}")
        };
        assert_eq!(key, "tool:msg_1:call_1");
        assert!(matches!(&patch.output, Some(ToolOutput::Terminal { stdout, .. }) if stdout.len() == OUTPUT_LIMIT - 1));
    }

    #[test]
    fn deltas_open_their_entry_once() {
        let mut mapper = Mapper::new(1);
        let mut caps = Capabilities::default();
        let delta = json!({"type": "session.text.delta",
            "data": {"assistantMessageID": "msg_1", "ordinal": 0, "delta": "a"}});
        let (first, _) = mapper.event(&delta, &mut caps, UNIX_EPOCH);
        let (second, _) = mapper.event(&delta, &mut caps, UNIX_EPOCH);
        assert_eq!(first.len(), 2);
        assert!(
            matches!(second.as_slice(), [Update::Append { key, text: appended }] if key == "text:msg_1:0" && appended == "a")
        );
    }
}
