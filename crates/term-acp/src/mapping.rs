//! How the agent's session updates change the thread (E4); `docs/acp.md` has the table this follows. Streamed text
//! goes into one entry until something else arrives; tool calls are keyed by the agent's id for them; what the
//! thread model has no place for is ignored, never guessed at.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use agent_client_protocol::v1::schema as acp;
use serde_json::Value;
use term_thread::{
    AgentMessage, EntryKind, ErrorKind, Lane, Notice, PermissionOption, PermissionRequest, RequestState, RuleScope,
    Status, TextEdit, Thought, Todo, TodoStatus, ToolCall, ToolKind, ToolOutput, ToolPatch, ToolStatus, ToolView,
    Update, UserMessage,
};

use crate::capabilities::{Capabilities, Category, Learn};
use crate::diff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Streaming {
    Message,
    Thought,
}

/// Counts mappers, so that one taking over a thread another one filled never reuses its keys.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Turns what the agent reports into changes of the thread.
#[derive(Debug, Default)]
pub struct Mapper {
    /// This mapper's part of every key.
    generation: u64,
    /// The entry streaming text now: its key, what it is and the agent's id for the message.
    open: Option<(String, Streaming, Option<String>)>,
    /// A user message the agent replays while loading a session: its key and its text so far.
    user: Option<(String, String)>,
    /// A prompt the user sent is being answered; the thread shows it already, so the agent's echo of it is left out.
    prompting: bool,
    next: u64,
    turn: u64,
    /// The tool calls so far by the agent's id: their name, title and, until they end, input, which a permission request
    /// may leave out.
    tools: HashMap<String, (String, String, Value)>,
}

fn put(key: String, at: SystemTime, entry: EntryKind) -> Update {
    Update::Put {
        lane: Lane::Main,
        key: Some(key),
        at,
        entry,
    }
}

/// What a content block reads as in markdown; images and audio have no text.
fn block_text(block: &acp::ContentBlock) -> Option<String> {
    match block {
        acp::ContentBlock::Text(text) => Some(text.text.clone()),
        acp::ContentBlock::ResourceLink(link) => Some(format!(
            "[{}]({})",
            link.title.as_deref().unwrap_or(&link.name),
            link.uri
        )),
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(text) => Some(format!("[{0}]({0})", text.uri)),
            _ => None,
        },
        _ => None,
    }
}

fn tool_kind(kind: &acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        acp::ToolKind::SwitchMode => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

fn tool_status(status: &acp::ToolCallStatus) -> ToolStatus {
    match status {
        acp::ToolCallStatus::InProgress => ToolStatus::InProgress,
        acp::ToolCallStatus::Completed => ToolStatus::Completed,
        acp::ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Pending,
    }
}

fn field<'a>(input: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| input.get(*n).and_then(Value::as_str))
}

/// The input field by field, as far as the kind and the input say it.
fn view(
    kind: ToolKind,
    input: &Value,
    content: &[acp::ToolCallContent],
    locations: &[acp::ToolCallLocation],
) -> ToolView {
    let diff = content.iter().find_map(|c| match c {
        acp::ToolCallContent::Diff(d) => Some(d),
        _ => None,
    });
    if let Some(diff) = diff {
        let path = diff.path.to_string_lossy().into_owned();
        return match &diff.old_text {
            Some(old) => ToolView::Edit {
                path,
                edits: vec![TextEdit {
                    old: old.clone(),
                    new: diff.new_text.clone(),
                    replace_all: false,
                }],
            },
            None => ToolView::Write {
                path,
                content: diff.new_text.clone(),
            },
        };
    }
    let location = locations.first();
    match kind {
        ToolKind::Execute => {
            let command = match input.get("command") {
                Some(Value::String(c)) => Some(c.clone()),
                Some(Value::Array(parts)) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
                _ => None,
            };
            command.map_or(ToolView::Other, |command| ToolView::Command {
                command,
                description: field(input, &["description"]).map(str::to_string),
                background: false,
                timeout: None,
            })
        }
        ToolKind::Read => field(input, &["path", "file_path", "abs_path"])
            .map(str::to_string)
            .or_else(|| location.map(|l| l.path.to_string_lossy().into_owned()))
            .map_or(ToolView::Other, |path| ToolView::Read {
                path,
                offset: location.and_then(|l| l.line).map(u64::from),
                limit: None,
            }),
        ToolKind::Search => field(input, &["pattern", "query"]).map_or(ToolView::Other, |pattern| ToolView::Search {
            pattern: pattern.to_string(),
            path: field(input, &["path"]).map(str::to_string),
            glob: field(input, &["glob", "include"]).map(str::to_string),
            results: None,
        }),
        ToolKind::Fetch => field(input, &["url"]).map_or(ToolView::Other, |url| ToolView::Fetch {
            url: url.to_string(),
            prompt: field(input, &["prompt"]).map(str::to_string),
        }),
        _ => ToolView::Other,
    }
}

/// What the call returned: its diff, else its text, else its raw output.
fn output(content: &[acp::ToolCallContent], raw: Option<&Value>) -> Option<ToolOutput> {
    for item in content {
        if let acp::ToolCallContent::Diff(d) = item {
            return Some(ToolOutput::Diff {
                path: d.path.to_string_lossy().into_owned(),
                hunks: diff::hunks(d.old_text.as_deref().unwrap_or_default(), &d.new_text),
            });
        }
    }
    let text: Vec<String> = content
        .iter()
        .filter_map(|c| match c {
            acp::ToolCallContent::Content(c) => block_text(&c.content),
            _ => None,
        })
        .collect();
    if !text.is_empty() {
        return Some(ToolOutput::Text(text.join("\n")));
    }
    match raw? {
        Value::String(s) => Some(ToolOutput::Text(s.clone())),
        Value::Object(map) if map.contains_key("stdout") || map.contains_key("stderr") => Some(ToolOutput::Terminal {
            stdout: map
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stderr: map
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            interrupted: false,
        }),
        _ => None,
    }
}

fn todos(plan: &acp::Plan) -> Vec<Todo> {
    plan.entries
        .iter()
        .map(|e| Todo {
            content: e.content.clone(),
            active_form: None,
            status: match e.status {
                acp::PlanEntryStatus::InProgress => TodoStatus::InProgress,
                acp::PlanEntryStatus::Completed => TodoStatus::Completed,
                _ => TodoStatus::Pending,
            },
        })
        .collect()
}

/// The thread model's options for the agent's: allowing once or always, refusing; the agent's own names are kept
/// in the order given, which `option_index` maps back.
pub(crate) fn permission_options(options: &[acp::PermissionOption]) -> Vec<PermissionOption> {
    options
        .iter()
        .enumerate()
        .map(|(n, o)| match o.kind {
            acp::PermissionOptionKind::AllowOnce => PermissionOption::AllowOnce,
            acp::PermissionOptionKind::AllowAlways => PermissionOption::AllowAlways {
                suggestion: n,
                rule: o.name.clone(),
                scope: RuleScope::Other(String::new()),
            },
            _ => PermissionOption::Deny,
        })
        .collect()
}

/// Whether a tool call titled `title` with `input` runs a git command that destroys work (E27); agents name the
/// command in the input or, without one, in the title.
pub(crate) fn destructive(title: &str, input: &Value) -> bool {
    term_thread::shell::destructive_input(input).is_some()
        || (term_thread::shell::command_of(input).is_none() && term_thread::shell::destructive_git(title).is_some())
}

impl Mapper {
    /// Whether the tool call `call` asks about runs a git command that destroys work, as far as the mapper knows it.
    pub(crate) fn is_destructive(&self, call: &acp::ToolCallUpdate) -> bool {
        let id = call.tool_call_id.0.to_string();
        let known = self.tools.get(&id);
        let title = call
            .fields
            .title
            .as_deref()
            .or(known.map(|k| k.1.as_str()))
            .unwrap_or_default();
        let input = call.fields.raw_input.as_ref().or(known.map(|k| &k.2));
        destructive(title, input.unwrap_or(&Value::Null))
    }

    /// A mapper for a thread, new or one an earlier session filled.
    pub fn new() -> Mapper {
        Mapper {
            generation: GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
            ..Mapper::default()
        }
    }

    fn key(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{}-{}", self.generation, self.next)
    }

    /// Ends the text that streams now.
    pub(crate) fn settle(&mut self, at: SystemTime) -> Vec<Update> {
        self.user = None;
        self.open
            .take()
            .map(|(key, ..)| Update::Finish { key, text: None, at })
            .into_iter()
            .collect()
    }

    /// A prompt the user sent, which starts a turn.
    pub(crate) fn begin_turn(&mut self, message: &UserMessage, at: SystemTime) -> Vec<Update> {
        self.turn += 1;
        self.prompting = true;
        let mut out = self.settle(at);
        let key = self.key("user");
        out.push(put(key, at, EntryKind::UserMessage(message.clone())));
        out.push(Update::Status(Status::Running));
        out
    }

    /// The turn ended for `reason`, the agent's stop reason as it named it on the wire.
    pub(crate) fn end_turn(&mut self, reason: acp::StopReason, at: SystemTime) -> Vec<Update> {
        self.prompting = false;
        let mut out = self.settle(at);
        out.push(Update::Status(Status::Idle));
        let notice = match reason {
            acp::StopReason::EndTurn => None,
            acp::StopReason::Cancelled => Some(Notice::Interrupted),
            other => Some(Notice::Error {
                kind: ErrorKind::Turn,
                message: wire_name(other),
            }),
        };
        if let Some(notice) = notice {
            let key = self.key("notice");
            out.push(put(key, at, EntryKind::Notice(notice)));
        }
        out
    }

    /// A failure to show in the thread, such as a prompt the agent rejected.
    pub(crate) fn error(&mut self, kind: ErrorKind, message: String, at: SystemTime) -> Vec<Update> {
        self.prompting = false;
        let mut out = self.settle(at);
        let key = self.key("notice");
        out.push(put(key, at, EntryKind::Notice(Notice::Error { kind, message })));
        out
    }

    fn stream(&mut self, kind: Streaming, chunk: &acp::ContentChunk, at: SystemTime) -> Vec<Update> {
        let Some(text) = block_text(&chunk.content) else {
            return Vec::new();
        };
        let id = chunk.message_id.as_ref().map(|m| m.0.to_string());
        let same = matches!(&self.open, Some((_, k, open_id)) if *k == kind && (id.is_none() || *open_id == id));
        let mut out = Vec::new();
        if !same {
            out = self.settle(at);
            let key = self.key(if kind == Streaming::Message {
                "message"
            } else {
                "thought"
            });
            let entry = match kind {
                Streaming::Message => EntryKind::AgentMessage(AgentMessage {
                    markdown: String::new(),
                    streaming: true,
                }),
                Streaming::Thought => EntryKind::Thought(Thought {
                    text: String::new(),
                    duration: None,
                    streaming: true,
                }),
            };
            out.push(put(key.clone(), at, entry));
            self.open = Some((key, kind, id));
        }
        if let Some((key, ..)) = &self.open {
            out.push(Update::Append { key: key.clone(), text });
        }
        out
    }

    fn replayed_user(&mut self, chunk: &acp::ContentChunk, at: SystemTime) -> Vec<Update> {
        let Some(text) = block_text(&chunk.content).filter(|_| !self.prompting) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if self.user.is_none() {
            out = self.settle(at);
            let key = self.key("user");
            self.user = Some((key, String::new()));
        }
        let Some((key, so_far)) = self.user.as_mut() else {
            return out;
        };
        so_far.push_str(&text);
        let message = UserMessage {
            text: so_far.clone(),
            ..UserMessage::default()
        };
        out.push(put(key.clone(), at, EntryKind::UserMessage(message)));
        out
    }

    fn tool_call(&mut self, call: &acp::ToolCall, at: SystemTime) -> Vec<Update> {
        let mut out = self.settle(at);
        let id = call.tool_call_id.0.to_string();
        let kind = tool_kind(&call.kind);
        let input = call.raw_input.clone().unwrap_or(Value::Null);
        let status = tool_status(&call.status);
        let name = call.name.clone().unwrap_or_else(|| call.title.clone());
        let ended = matches!(status, ToolStatus::Completed | ToolStatus::Failed);
        let kept = if ended { Value::Null } else { input.clone() };
        self.tools.insert(id.clone(), (name.clone(), call.title.clone(), kept));
        let mut entry = ToolCall::new(id.clone(), name, kind);
        entry.title = call.title.clone();
        entry.status = status;
        entry.view = view(kind, &input, &call.content, &call.locations);
        entry.output = output(&call.content, call.raw_output.as_ref());
        entry.input = input;
        if status != ToolStatus::Pending {
            entry.started = Some(at);
        }
        if ended {
            entry.ended = Some(at);
        }
        out.push(put(id, at, EntryKind::ToolCall(entry)));
        out
    }

    fn tool_update(&mut self, update: &acp::ToolCallUpdate, at: SystemTime) -> Vec<Update> {
        let id = update.tool_call_id.0.to_string();
        let fields = &update.fields;
        if !self.tools.contains_key(&id) {
            let mut call = acp::ToolCall::new(update.tool_call_id.clone(), fields.title.clone().unwrap_or_default());
            call.kind = fields.kind.unwrap_or_default();
            call.status = fields.status.unwrap_or_default();
            call.name.clone_from(&fields.name);
            call.content = fields.content.clone().unwrap_or_default();
            call.locations = fields.locations.clone().unwrap_or_default();
            call.raw_input.clone_from(&fields.raw_input);
            call.raw_output.clone_from(&fields.raw_output);
            return self.tool_call(&call, at);
        }
        let mut out = self.settle(at);
        if let Some(known) = self.tools.get_mut(&id) {
            if let Some(name) = &fields.name {
                known.0.clone_from(name);
            }
            if let Some(title) = &fields.title {
                known.1.clone_from(title);
            }
            if let Some(input) = &fields.raw_input {
                known.2.clone_from(input);
            }
        }
        let status = fields.status.as_ref().map(tool_status);
        if matches!(status, Some(ToolStatus::Completed | ToolStatus::Failed))
            && let Some(known) = self.tools.get_mut(&id)
        {
            known.2 = Value::Null;
        }
        let content = fields.content.as_deref().unwrap_or_default();
        let kind = fields.kind.as_ref().map(tool_kind);
        let input = fields.raw_input.clone();
        let view = (input.is_some() || !content.is_empty()).then(|| {
            view(
                kind.unwrap_or(ToolKind::Other),
                input.as_ref().unwrap_or(&Value::Null),
                content,
                fields.locations.as_deref().unwrap_or_default(),
            )
        });
        let patch = ToolPatch {
            input,
            title: fields.title.clone(),
            view: view.filter(|v| *v != ToolView::Other),
            status,
            output: output(content, fields.raw_output.as_ref()),
            started: (status == Some(ToolStatus::InProgress)).then_some(at),
            ended: matches!(status, Some(ToolStatus::Completed | ToolStatus::Failed)).then_some(at),
            ..ToolPatch::default()
        };
        out.push(Update::Tool { key: id, patch });
        out
    }

    /// The agent asks to run a tool call; `request` is the app's id for the question.
    pub(crate) fn permission(
        &mut self,
        request: &str,
        call: &acp::ToolCallUpdate,
        options: &[acp::PermissionOption],
        at: SystemTime,
    ) -> Vec<Update> {
        let mut out = self.tool_update(call, at);
        let fields = &call.fields;
        let id = call.tool_call_id.0.to_string();
        let (name, title, input) = self.tools.get(&id).cloned().unwrap_or_default();
        let destructive = destructive(&title, &input);
        let entry = PermissionRequest {
            request_id: request.to_string(),
            tool_call: Some(id),
            tool_name: name,
            title: Some(title).filter(|t| !t.is_empty()),
            description: None,
            reason: None,
            blocked_path: fields
                .locations
                .as_ref()
                .and_then(|l| l.first())
                .map(|l| l.path.to_string_lossy().into_owned()),
            input,
            options: permission_options(options),
            default_to_no: destructive,
            state: RequestState::Pending,
        };
        out.push(put(
            format!("permission-{request}"),
            at,
            EntryKind::PermissionRequest(entry),
        ));
        out.push(Update::Status(Status::RequiresAction));
        out
    }

    /// Maps one session update; `caps` learns what the session now offers, and the flag says whether it changed.
    pub(crate) fn update(
        &mut self,
        update: &acp::SessionUpdate,
        caps: &mut Capabilities,
        at: SystemTime,
    ) -> (Vec<Update>, bool) {
        let updates = match update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => self.stream(Streaming::Message, chunk, at),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => self.stream(Streaming::Thought, chunk, at),
            acp::SessionUpdate::UserMessageChunk(chunk) => self.replayed_user(chunk, at),
            acp::SessionUpdate::ToolCall(call) => self.tool_call(call, at),
            acp::SessionUpdate::ToolCallUpdate(call) => self.tool_update(call, at),
            acp::SessionUpdate::Plan(plan) => {
                let list = todos(plan);
                let key = format!("plan-{}-{}", self.generation, self.turn);
                let mut out = self.settle(at);
                out.push(Update::Todos(list.clone()));
                out.push(put(key, at, EntryKind::Todos(list)));
                out
            }
            acp::SessionUpdate::SessionInfoUpdate(info) => match &info.title {
                agent_client_protocol::schema::MaybeUndefined::Value(title) => vec![Update::Title(title.clone())],
                _ => Vec::new(),
            },
            acp::SessionUpdate::UsageUpdate(usage) => vec![crate::usage::context(usage)],
            acp::SessionUpdate::AvailableCommandsUpdate(commands) => {
                caps.learn_commands(&commands.available_commands);
                return (Vec::new(), true);
            }
            acp::SessionUpdate::CurrentModeUpdate(mode) => {
                let id = mode.current_mode_id.0.to_string();
                caps.mode = Some(id.clone());
                return (vec![Update::Mode(id)], true);
            }
            acp::SessionUpdate::ConfigOptionUpdate(options) => {
                caps.learn_options(&options.config_options);
                return (self.options_changed(caps), true);
            }
            _ => Vec::new(),
        };
        (updates, false)
    }

    /// The thread's model, thought level and mode as the session's options now set them, by the agent's ids.
    pub(crate) fn options_changed(&self, caps: &Capabilities) -> Vec<Update> {
        let mut out = Vec::new();
        if let Some(model) = caps.model() {
            out.push(Update::Model(model.current_value()));
        }
        if let Some(level) = caps.thought_level() {
            out.push(Update::Effort(Some(level.current_value())));
        }
        if let Some(mode) = caps.options.iter().find(|o| o.category == Category::Mode) {
            out.push(Update::Mode(mode.current_value()));
        }
        out
    }

    /// Maps the parameters of a `session/update` notification as JSON; what the protocol does not define is ignored.
    pub fn apply_json(&mut self, params: &Value, caps: &mut Capabilities, at: SystemTime) -> (Vec<Update>, bool) {
        match serde_json::from_value::<acp::SessionNotification>(params.clone()) {
            Ok(notification) => self.update(&notification.update, caps, at),
            Err(_) => (Vec::new(), false),
        }
    }

    /// Maps the answer to `session/prompt` as JSON, such as `{"stopReason": "max_tokens"}`, which ends the turn; an
    /// answer of another shape changes nothing.
    pub fn prompt_answered_json(&mut self, response: &Value, at: SystemTime) -> Vec<Update> {
        match serde_json::from_value::<acp::PromptResponse>(response.clone()) {
            Ok(response) => self.end_turn(response.stop_reason, at),
            Err(_) => Vec::new(),
        }
    }
}

/// The protocol's name for `reason`, such as `max_tokens`.
fn wire_name(reason: acp::StopReason) -> String {
    serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_destructive_git_command_is_found_in_the_input_or_else_the_title() {
        assert!(destructive("Run command", &json!({"command": "git push --force"})));
        assert!(destructive(
            "Run command",
            &json!({"command": ["git", "clean", "-fdx"]})
        ));
        assert!(destructive("`git -C x reset --hard`", &Value::Null));
        assert!(!destructive("git reset --hard", &json!({"command": "git status"})));
        assert!(!destructive("Read notes.md", &json!({"path": "notes.md"})));
    }

    #[test]
    fn a_request_about_a_destructive_git_command_preselects_no_allowing() {
        let mut mapper = Mapper::new();
        let call: acp::ToolCallUpdate = serde_json::from_value(json!({
            "toolCallId": "c1", "title": "git reset --hard", "kind": "execute", "status": "pending",
            "rawInput": {"command": "git reset --hard HEAD~1"}
        }))
        .unwrap();
        let options: Vec<acp::PermissionOption> = serde_json::from_value(json!([
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"}
        ]))
        .unwrap();
        let updates = mapper.permission("p1", &call, &options, SystemTime::UNIX_EPOCH);
        let request = updates.iter().find_map(|u| match u {
            Update::Put {
                entry: EntryKind::PermissionRequest(r),
                ..
            } => Some(r.clone()),
            _ => None,
        });
        assert!(request.expect("a permission request").default_to_no);
        assert!(mapper.is_destructive(&call));
    }

    /// The thread after the session updates `updates`, given as their JSON.
    fn thread_after(updates: &[Value]) -> (term_thread::Thread, Capabilities) {
        let mut mapper = Mapper::new();
        let mut caps = Capabilities::default();
        let mut thread = term_thread::Thread::new(term_thread::ThreadId::new(), "acp", "/w");
        for update in updates {
            let params = json!({"sessionId": "s-1", "update": update});
            let (changes, _) = mapper.apply_json(&params, &mut caps, SystemTime::UNIX_EPOCH);
            thread.apply_all(changes);
        }
        (thread, caps)
    }

    fn call(thread: &term_thread::Thread, id: &str) -> ToolCall {
        match &thread.entry_by_key(id).map(|e| &e.kind) {
            Some(EntryKind::ToolCall(call)) => call.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn texts(thread: &term_thread::Thread) -> Vec<String> {
        thread
            .entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::AgentMessage(m) => Some(m.markdown.clone()),
                EntryKind::Thought(t) => Some(format!("thought: {}", t.text)),
                EntryKind::UserMessage(m) => Some(format!("user: {}", m.text)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn links_and_resources_read_as_markdown_links_and_images_add_nothing() {
        let chunk = |kind: &str, content: Value| json!({"sessionUpdate": kind, "content": content});
        let (thread, _) = thread_after(&[
            chunk("agent_thought_chunk", json!({"type": "text", "text": "Looking."})),
            chunk("agent_message_chunk", json!({"type": "text", "text": "See "})),
            chunk(
                "agent_message_chunk",
                json!({"type": "resource_link", "name": "a.rs", "uri": "file:///w/a.rs"}),
            ),
            chunk(
                "agent_message_chunk",
                json!({"type": "resource_link", "name": "b", "title": "B file", "uri": "file:///w/b"}),
            ),
            chunk(
                "agent_message_chunk",
                json!({"type": "resource", "resource": {"uri": "file:///w/c.md", "text": "# C"}}),
            ),
            chunk(
                "agent_message_chunk",
                json!({"type": "resource", "resource": {"uri": "file:///w/d.bin", "blob": "AAEC"}}),
            ),
            chunk(
                "agent_message_chunk",
                json!({"type": "image", "data": "iVBORw==", "mimeType": "image/png"}),
            ),
            chunk(
                "user_message_chunk",
                json!({"type": "text", "text": "an earlier prompt"}),
            ),
        ]);
        assert_eq!(
            texts(&thread),
            [
                "thought: Looking.",
                "See [a.rs](file:///w/a.rs)[B file](file:///w/b)[file:///w/c.md](file:///w/c.md)",
                "user: an earlier prompt"
            ]
        );
    }

    /// A `tool_call` update for the call `id` of `kind` with `input` and the further fields `extra`.
    fn tool(id: &str, kind: &str, input: Value, extra: Value) -> Value {
        let mut call = json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": id, "kind": kind,
                              "status": "in_progress", "rawInput": input});
        call.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        call
    }

    #[test]
    fn each_kind_of_tool_call_shows_its_input_field_by_field() {
        let (thread, _) = thread_after(&[
            tool(
                "run",
                "execute",
                json!({"command": ["cargo", "test"], "description": "tests"}),
                json!({}),
            ),
            tool(
                "read",
                "read",
                json!({}),
                json!({"locations": [{"path": "/w/a.rs", "line": 40}]}),
            ),
            tool(
                "find",
                "search",
                json!({"query": "fn main", "path": "src", "include": "*.rs"}),
                json!({}),
            ),
            tool(
                "get",
                "fetch",
                json!({"url": "https://example.test", "prompt": "the title"}),
                json!({}),
            ),
            tool(
                "new",
                "edit",
                json!({}),
                json!({"content": [{"type": "diff", "path": "/w/n.rs", "newText": "fn n() {}\n"}]}),
            ),
        ]);
        assert_eq!(
            call(&thread, "run").view,
            ToolView::Command {
                command: "cargo test".into(),
                description: Some("tests".into()),
                background: false,
                timeout: None
            }
        );
        assert_eq!(
            call(&thread, "read").view,
            ToolView::Read {
                path: "/w/a.rs".into(),
                offset: Some(40),
                limit: None
            }
        );
        assert_eq!(
            call(&thread, "find").view,
            ToolView::Search {
                pattern: "fn main".into(),
                path: Some("src".into()),
                glob: Some("*.rs".into()),
                results: None
            }
        );
        assert_eq!(
            call(&thread, "get").view,
            ToolView::Fetch {
                url: "https://example.test".into(),
                prompt: Some("the title".into())
            }
        );
        let new = call(&thread, "new");
        assert_eq!(
            new.view,
            ToolView::Write {
                path: "/w/n.rs".into(),
                content: "fn n() {}\n".into()
            }
        );
        assert!(matches!(new.output, Some(ToolOutput::Diff { ref path, .. }) if path == "/w/n.rs"));
    }

    #[test]
    fn each_kind_of_tool_call_keeps_its_kind_and_its_output_in_the_shape_it_came() {
        let (thread, _) = thread_after(&[
            tool(
                "rm",
                "delete",
                json!({"path": "/w/x"}),
                json!({"status": "completed", "rawOutput": "removed"}),
            ),
            tool(
                "mv",
                "move",
                json!({}),
                json!({"status": "failed", "rawOutput": {"stdout": "", "stderr": "denied"}}),
            ),
            tool("plan", "think", json!({}), json!({"rawOutput": {"steps": 3}})),
            tool(
                "mode",
                "switch_mode",
                json!({}),
                json!({"content": [{"type": "content", "content": {"type": "text", "text": "now in plan mode"}}]}),
            ),
            tool("odd", "other", json!({}), json!({})),
        ]);
        let rm = call(&thread, "rm");
        assert_eq!((rm.kind, rm.status), (ToolKind::Delete, ToolStatus::Completed));
        assert_eq!(rm.output, Some(ToolOutput::Text("removed".into())));
        assert!(rm.ended.is_some());
        let mv = call(&thread, "mv");
        assert_eq!((mv.kind, mv.status), (ToolKind::Move, ToolStatus::Failed));
        assert_eq!(
            mv.output,
            Some(ToolOutput::Terminal {
                stdout: String::new(),
                stderr: "denied".into(),
                interrupted: false
            })
        );
        assert_eq!(call(&thread, "plan").kind, ToolKind::Think);
        assert_eq!(
            call(&thread, "plan").output,
            None,
            "an output of no known shape is left out"
        );
        assert_eq!(call(&thread, "mode").kind, ToolKind::SwitchMode);
        assert_eq!(
            call(&thread, "mode").output,
            Some(ToolOutput::Text("now in plan mode".into()))
        );
        assert_eq!(
            (call(&thread, "odd").kind, call(&thread, "odd").view.clone()),
            (ToolKind::Other, ToolView::Other)
        );
    }

    #[test]
    fn an_update_renames_a_known_call_and_one_for_an_unknown_call_starts_it() {
        let (thread, _) = thread_after(&[
            json!({"sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Read", "kind": "read", "status": "pending"}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "title": "Read a.rs", "kind": "read",
                   "status": "completed", "rawInput": {"path": "a.rs"}}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c2", "title": "Fetch", "kind": "fetch",
                   "status": "in_progress", "rawInput": {"url": "https://example.test"}}),
        ]);
        let first = call(&thread, "c1");
        assert_eq!(
            (first.title.as_str(), first.status),
            ("Read a.rs", ToolStatus::Completed)
        );
        assert_eq!(
            first.view,
            ToolView::Read {
                path: "a.rs".into(),
                offset: None,
                limit: None
            }
        );
        let second = call(&thread, "c2");
        assert_eq!(
            (second.title.as_str(), second.status),
            ("Fetch", ToolStatus::InProgress)
        );
        assert!(second.started.is_some() && second.ended.is_none());
    }

    #[test]
    fn titles_modes_and_options_the_agent_announces_reach_the_thread() {
        let option = |id: &str, category: &str, current: &str| {
            json!({"id": id, "name": id, "category": category, "type": "select", "currentValue": current,
                   "options": [{"value": current, "name": current}]})
        };
        let (thread, caps) = thread_after(&[
            json!({"sessionUpdate": "session_info_update", "title": "Fix the parser"}),
            json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
            json!({"sessionUpdate": "config_option_update", "configOptions": [
                option("model", "model", "deep"), option("effort", "thought_level", "high"), option("mode", "mode", "code")
            ]}),
            json!({"sessionUpdate": "plan", "entries": [
                {"content": "parse", "priority": "high", "status": "completed"},
                {"content": "test", "priority": "medium", "status": "in_progress"},
                {"content": "ship", "priority": "low", "status": "pending"}
            ]}),
            json!({"sessionUpdate": "not_in_the_protocol"}),
        ]);
        assert_eq!(thread.title(), "Fix the parser");
        assert_eq!(
            (thread.model(), thread.effort(), thread.mode()),
            (Some("deep"), Some("high"), Some("code"))
        );
        assert_eq!(caps.mode.as_deref(), Some("plan"));
        let statuses: Vec<TodoStatus> = thread.todos().iter().map(|t| t.status).collect();
        assert_eq!(
            statuses,
            [TodoStatus::Completed, TodoStatus::InProgress, TodoStatus::Pending]
        );
    }

    #[test]
    fn a_turn_that_stops_early_or_fails_says_why() {
        let mut mapper = Mapper::new();
        let mut thread = term_thread::Thread::new(term_thread::ThreadId::new(), "acp", "/w");
        let at = SystemTime::UNIX_EPOCH;
        for reason in [
            acp::StopReason::EndTurn,
            acp::StopReason::Cancelled,
            acp::StopReason::MaxTokens,
        ] {
            thread.apply_all(mapper.begin_turn(&UserMessage::default(), at));
            thread.apply_all(mapper.end_turn(reason, at));
        }
        thread.apply_all(mapper.error(ErrorKind::Process, "the agent exited".into(), at));
        let notices: Vec<Notice> = thread
            .entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notices,
            [
                Notice::Interrupted,
                Notice::Error {
                    kind: ErrorKind::Turn,
                    message: "max_tokens".into()
                },
                Notice::Error {
                    kind: ErrorKind::Process,
                    message: "the agent exited".into()
                }
            ]
        );
        assert_eq!(thread.status(), Status::Idle);
    }
}
