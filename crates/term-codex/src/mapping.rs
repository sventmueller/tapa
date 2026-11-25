//! How the server's notifications change the thread (E147, B2); `docs/codex.md` has the table this follows. Items
//! are keyed by turn and item id, which Codex keeps unique, so a session continuing a thread another one filled never
//! overwrites its entries; what the thread model has no place for is ignored, never guessed at.

mod cards;
mod tasks;
mod usage;

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::Value;
use term_thread::{
    Activity, AgentMessage, EntryKind, ErrorKind, InfoLevel, Lane, Notice, Status, Thought, Todo, TodoStatus,
    ToolOutput, ToolPatch, Update, UserMessage,
};

use crate::capabilities::{Account, Capabilities};
use crate::protocol::{FileChange, Item, number, text};

pub(crate) use tasks::Stop;
pub(crate) use usage::{rate_limits, subscription as usage_subscription};

/// The most command output a card keeps while it streams; older output is dropped from the front.
const OUTPUT_LIMIT: usize = 512 * 1024;

/// What the mapper remembers of an item that has started and not completed.
#[derive(Debug, Clone, PartialEq)]
enum Open {
    /// An agent message or a proposed plan, streaming.
    Text,
    /// Reasoning, streaming its summary or, until one comes, its raw text.
    Thought { summary: bool, raw: bool, parts: u64 },
    /// A tool card, with the command output so far.
    Tool { output: String },
}

/// Turns what the server reports into changes of the thread.
#[derive(Debug, Default)]
pub struct Mapper {
    /// This mapper's part of the keys it makes up itself.
    origin: u64,
    next: u64,
    thread: Option<String>,
    turn: Option<String>,
    /// The turn that ended last, which a late answer to `turn/start` must not bring back.
    ended: Option<String>,
    /// The keys of the user messages this session sent in the running turn, which the server echoes as `clientId`.
    sent: Vec<String>,
    open: HashMap<String, Open>,
    /// An error notice was shown for the running turn, so its failed end adds no second one.
    failed: bool,
    /// The turn of the user message chained last, which the next one follows.
    chained: Option<String>,
    usage: usage::Tally,
    diff: Option<String>,
    tasks: tasks::Tasks,
}

pub(crate) fn put(key: String, at: SystemTime, entry: EntryKind) -> Update {
    Update::Put {
        lane: Lane::Main,
        key: Some(key),
        at,
        entry,
    }
}

/// The time of a `...AtMs` field of `value`, else `fallback`.
fn at_ms(value: &Value, name: &str, fallback: SystemTime) -> SystemTime {
    number(value, name).map_or(fallback, |ms| UNIX_EPOCH + Duration::from_millis(ms))
}

fn item_key(turn: &str, item: &str) -> String {
    format!("item:{turn}:{item}")
}

/// The text a user message item holds; images and mentions have none.
fn user_text(content: &[Value]) -> String {
    content
        .iter()
        .filter(|c| text(c, "type") == Some("text"))
        .filter_map(|c| text(c, "text"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The kind of error notice an error the server reports stands for.
pub(crate) fn error_kind(error: &Value) -> ErrorKind {
    let info = error.get("codexErrorInfo");
    let name = info.and_then(|i| {
        i.as_str()
            .or_else(|| i.as_object().and_then(|o| o.keys().next().map(String::as_str)))
    });
    match name {
        Some("unauthorized") => ErrorKind::Auth,
        Some("contextWindowExceeded" | "sessionBudgetExceeded" | "tooManyDenials" | "activeTurnNotSteerable") => {
            ErrorKind::Turn
        }
        Some("sandboxError") => ErrorKind::Other,
        _ => ErrorKind::Api,
    }
}

/// The message of an error the server reports, with its details.
fn error_message(error: &Value) -> String {
    let message = text(error, "message").unwrap_or_default();
    match text(error, "additionalDetails").filter(|d| !d.is_empty()) {
        Some(details) => format!("{message}\n{details}"),
        None => message.to_string(),
    }
}

fn todos(plan: &Value) -> Vec<Todo> {
    plan.as_array()
        .into_iter()
        .flatten()
        .filter_map(|step| {
            Some(Todo {
                content: text(step, "step")?.to_string(),
                active_form: None,
                status: match text(step, "status") {
                    Some("inProgress") => TodoStatus::InProgress,
                    Some("completed") => TodoStatus::Completed,
                    _ => TodoStatus::Pending,
                },
            })
        })
        .collect()
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

    /// Takes notifications for `thread` and its subagents; unrelated threads are left out.
    pub fn set_thread(&mut self, thread: &str) {
        self.thread = Some(thread.to_string());
    }

    /// The latest unified diff of everything the running or last turn changed.
    pub fn diff(&self) -> Option<&str> {
        self.diff.as_deref()
    }

    /// The running turn's id, once the server named it.
    pub fn turn(&self) -> Option<&str> {
        self.turn.as_deref()
    }

    /// The id `turn/start` answered with, unless the turn's notifications came first.
    pub(crate) fn turn_answered(&mut self, turn: &str) {
        if self.turn.is_none() && self.ended.as_deref() != Some(turn) {
            self.turn = Some(turn.to_string());
        }
    }

    pub(crate) fn key(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{:x}-{}", self.origin, self.next)
    }

    /// Ends the text and reasoning that stream now.
    pub(crate) fn settle(&mut self, at: SystemTime) -> Vec<Update> {
        let mut keys: Vec<String> = self
            .open
            .iter()
            .filter(|(_, open)| !matches!(open, Open::Tool { .. }))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys.into_iter()
            .map(|key| {
                self.open.remove(&key);
                Update::Finish { key, text: None, at }
            })
            .collect()
    }

    /// A message the user sent, which starts a turn; the key it returns goes out as `clientUserMessageId`.
    pub(crate) fn begin_turn(&mut self, message: &UserMessage, at: SystemTime) -> (String, Vec<Update>) {
        let key = self.key("user");
        self.sent = vec![key.clone()];
        self.failed = false;
        let mut out = self.settle(at);
        out.push(put(key.clone(), at, EntryKind::UserMessage(message.clone())));
        out.push(Update::Status(Status::Running));
        (key, out)
    }

    /// Input added to the running turn.
    pub(crate) fn steered(&mut self, message: &UserMessage, at: SystemTime) -> (String, Vec<Update>) {
        let key = self.key("user");
        self.sent.push(key.clone());
        (key.clone(), vec![put(key, at, EntryKind::UserMessage(message.clone()))])
    }

    /// The turn is over without the server saying so, such as when `turn/start` failed or the process ended.
    pub(crate) fn abandon(&mut self, at: SystemTime) -> Vec<Update> {
        self.turn = None;
        self.sent.clear();
        let mut out = self.settle(at);
        out.push(Update::Status(Status::Idle));
        out
    }

    /// A failure to show in the thread, such as a turn the server refused.
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

    /// The conversation of a resumed thread, as `thread/resume` answered it: what the thread showed goes first.
    pub(crate) fn history(&mut self, thread: &Value, at: SystemTime) -> Vec<Update> {
        self.tasks = tasks::Tasks::default();
        let mut out = vec![Update::Clear];
        for turn in thread.get("turns").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = text(turn, "id") else {
                continue;
            };
            let started = number(turn, "startedAt").map_or(at, |s| UNIX_EPOCH + Duration::from_secs(s));
            for item in turn.get("items").and_then(Value::as_array).into_iter().flatten() {
                let item_at = at_ms(item, "startedAtMs", started);
                let ended = at_ms(item, "completedAtMs", item_at);
                if let Some(parsed) = Item::read(item) {
                    out.extend(self.item_started(&parsed, id, item_at));
                    out.extend(self.item_completed(&parsed, id, ended));
                    out.extend(self.tasks.item(&parsed, self.thread.as_deref(), ended));
                }
            }
            if text(turn, "status") == Some("failed")
                && let Some(error) = turn.get("error").filter(|e| !e.is_null())
            {
                out.extend(self.error(error_kind(error), error_message(error), started));
            }
        }
        out.extend(self.settle(at));
        if let Some(name) = text(thread, "name").filter(|n| !n.is_empty()) {
            out.push(Update::Title(name.to_string()));
        }
        out
    }

    /// Maps one notification; `caps` learns what changed about the session, and the flag says whether it did.
    pub fn notification(
        &mut self,
        method: &str,
        params: &Value,
        caps: &mut Capabilities,
        at: SystemTime,
    ) -> (Vec<Update>, bool) {
        if method == "thread/started" {
            return (self.tasks.started(params, self.thread.as_deref(), at), false);
        }
        if let (Some(ours), Some(theirs)) = (&self.thread, text(params, "threadId"))
            && ours != theirs
        {
            return (self.tasks.notification(method, params, at), false);
        }
        let (mut updates, learned) = self.own_notification(method, params, caps, at);
        if matches!(method, "item/started" | "item/completed")
            && let Some(item) = params.get("item").and_then(Item::read)
        {
            updates.extend(self.tasks.item(&item, self.thread.as_deref(), at));
        }
        updates.extend(self.tasks.observe(method, params, at));
        (updates, learned)
    }

    fn own_notification(
        &mut self,
        method: &str,
        params: &Value,
        caps: &mut Capabilities,
        at: SystemTime,
    ) -> (Vec<Update>, bool) {
        let updates = match method {
            "turn/started" => self.turn_started(params),
            "turn/completed" => self.turn_completed(params, at),
            "item/started" => self.item_notification(params, "startedAtMs", at, true),
            "item/completed" => self.item_notification(params, "completedAtMs", at, false),
            "item/agentMessage/delta" | "item/plan/delta" => self.text_delta(params, at),
            "item/reasoning/summaryTextDelta" => self.thought_delta(params, true, at),
            "item/reasoning/textDelta" => self.thought_delta(params, false, at),
            "item/reasoning/summaryPartAdded" => self.thought_part(params),
            "item/commandExecution/outputDelta" => self.output_delta(params),
            "item/fileChange/patchUpdated" => self.patch_updated(params),
            "turn/plan/updated" => self.plan(params, at),
            "turn/diff/updated" => {
                self.diff = text(params, "diff").map(str::to_string);
                Vec::new()
            }
            "thread/tokenUsage/updated" => self.usage.token_usage(params),
            "account/rateLimits/updated" => rate_limits(params.get("rateLimits").unwrap_or(&Value::Null), at),
            "thread/name/updated" => text(params, "threadName")
                .map(|name| Update::Title(name.to_string()))
                .into_iter()
                .collect(),
            "error" => self.error_notification(params, at),
            "model/rerouted" => return self.rerouted(params, caps, at),
            "account/updated" => return (account_updated(params, caps), true),
            "warning" | "configWarning" => self.warning(params, InfoLevel::Warning, at),
            "deprecationNotice" => self.warning(params, InfoLevel::Notice, at),
            _ => Vec::new(),
        };
        (updates, false)
    }

    fn turn_started(&mut self, params: &Value) -> Vec<Update> {
        let turn = params.get("turn").unwrap_or(&Value::Null);
        self.turn = text(turn, "id").map(str::to_string);
        self.usage.turn_started();
        vec![Update::Status(Status::Running)]
    }

    fn turn_completed(&mut self, params: &Value, at: SystemTime) -> Vec<Update> {
        let turn = params.get("turn").unwrap_or(&Value::Null);
        let mut out = self.settle(at);
        match text(turn, "status") {
            Some("interrupted") => {
                let key = self.key("notice");
                out.push(put(key, at, EntryKind::Notice(Notice::Interrupted)));
            }
            Some("failed") if !self.failed => {
                let error = turn.get("error").unwrap_or(&Value::Null);
                let message = Some(error_message(error))
                    .filter(|m| !m.is_empty())
                    .unwrap_or_else(|| "failed".into());
                out.extend(self.error(error_kind(error), message, at));
            }
            _ => {}
        }
        let duration = number(turn, "durationMs").map(Duration::from_millis);
        out.extend(self.usage.turn_completed(duration));
        out.push(Update::Status(Status::Idle));
        self.ended = self.turn.take().or_else(|| text(turn, "id").map(str::to_string));
        self.sent.clear();
        self.failed = false;
        out
    }

    fn item_notification(&mut self, params: &Value, time: &str, at: SystemTime, started: bool) -> Vec<Update> {
        let (Some(turn), Some(item)) = (text(params, "turnId"), params.get("item")) else {
            return Vec::new();
        };
        let Some(parsed) = Item::read(item) else {
            return Vec::new();
        };
        let at = at_ms(params, time, at);
        if started {
            self.item_started(&parsed, turn, at)
        } else {
            self.item_completed(&parsed, turn, at)
        }
    }

    fn user_item(
        &mut self,
        key: String,
        turn: &str,
        content: &[Value],
        client: Option<&str>,
        at: SystemTime,
    ) -> Vec<Update> {
        if let Some(sent) = client.filter(|c| self.sent.iter().any(|s| s == c)) {
            return vec![self.chain(sent.to_string(), turn)];
        }
        let message = UserMessage {
            text: user_text(content),
            ..UserMessage::default()
        };
        let key = client.map_or(key, |c| format!("user:{c}"));
        vec![
            put(key.clone(), at, EntryKind::UserMessage(message)),
            self.chain(key, turn),
        ]
    }

    /// Names the turn the user message `key` began, which `thread/revert` takes to drop it and every later one.
    fn chain(&mut self, key: String, turn: &str) -> Update {
        let follows = self.chained.clone().filter(|last| last != turn).unwrap_or_default();
        self.chained = Some(turn.to_string());
        Update::Chain {
            key,
            uuid: turn.to_string(),
            follows: Some(follows),
        }
    }

    fn item_started(&mut self, item: &Item, turn: &str, at: SystemTime) -> Vec<Update> {
        let Some(key) = item.id().map(|id| item_key(turn, id)) else {
            return Vec::new();
        };
        match item {
            Item::CollabAgentToolCall { tool, .. } if tool == "spawnAgent" => Vec::new(),
            Item::UserMessage { content, client_id, .. } => {
                self.user_item(key, turn, content, client_id.as_deref(), at)
            }
            Item::AgentMessage { text, .. } | Item::Plan { text, .. } => {
                self.open.insert(key.clone(), Open::Text);
                let entry = EntryKind::AgentMessage(AgentMessage {
                    markdown: text.clone(),
                    streaming: true,
                });
                vec![put(key, at, entry)]
            }
            Item::Reasoning { .. } => {
                self.open.insert(
                    key.clone(),
                    Open::Thought {
                        summary: false,
                        raw: false,
                        parts: 0,
                    },
                );
                vec![put(
                    key,
                    at,
                    EntryKind::Thought(Thought {
                        streaming: true,
                        ..Thought::default()
                    }),
                )]
            }
            Item::ContextCompaction { .. } => vec![Update::Activity(Some(Activity::Compacting))],
            Item::EnteredReviewMode { .. } | Item::ExitedReviewMode { .. } | Item::Other => Vec::new(),
            tool => match cards::card(tool) {
                Some(card) => {
                    let ended = card.is_over();
                    self.open.insert(key.clone(), Open::Tool { output: String::new() });
                    vec![put(key, at, EntryKind::ToolCall(card.call(at, ended)))]
                }
                None => Vec::new(),
            },
        }
    }

    fn item_completed(&mut self, item: &Item, turn: &str, at: SystemTime) -> Vec<Update> {
        let Some(key) = item.id().map(|id| item_key(turn, id)) else {
            return Vec::new();
        };
        let known = self.open.remove(&key);
        match item {
            Item::CollabAgentToolCall { tool, .. } if tool == "spawnAgent" => Vec::new(),
            Item::UserMessage { content, client_id, .. } => {
                self.user_item(key, turn, content, client_id.as_deref(), at)
            }
            Item::AgentMessage { text, .. } | Item::Plan { text, .. } => {
                let mut out = Vec::new();
                if known.is_none() {
                    out.push(put(key.clone(), at, EntryKind::AgentMessage(AgentMessage::default())));
                }
                out.push(Update::Finish {
                    key,
                    text: Some(text.clone()),
                    at,
                });
                out
            }
            Item::Reasoning { summary, content, .. } => {
                let mut out = Vec::new();
                if known.is_none() {
                    out.push(put(key.clone(), at, EntryKind::Thought(Thought::default())));
                }
                let full = if summary.is_empty() { content } else { summary };
                let text = Some(full.join("\n\n")).filter(|t| !t.is_empty());
                out.push(Update::Finish { key, text, at });
                out
            }
            Item::ContextCompaction { .. } => {
                let notice = Notice::Compaction {
                    manual: false,
                    tokens_before: self.usage.context_used().unwrap_or_default(),
                    tokens_after: None,
                };
                vec![Update::Activity(None), put(key, at, EntryKind::Notice(notice))]
            }
            Item::EnteredReviewMode { review, .. } | Item::ExitedReviewMode { review, .. } => {
                vec![put(
                    key,
                    at,
                    EntryKind::Notice(Notice::Info {
                        level: InfoLevel::Notice,
                        text: review.clone(),
                    }),
                )]
            }
            Item::Other => Vec::new(),
            tool => match (cards::card(tool), known) {
                (Some(card), Some(_)) => vec![Update::Tool {
                    key,
                    patch: card.patch(at),
                }],
                (Some(card), None) => vec![put(key, at, EntryKind::ToolCall(card.call(at, true)))],
                (None, _) => Vec::new(),
            },
        }
    }

    fn item_ref(params: &Value) -> Option<String> {
        Some(item_key(text(params, "turnId")?, text(params, "itemId")?))
    }

    fn text_delta(&mut self, params: &Value, at: SystemTime) -> Vec<Update> {
        let (Some(key), Some(delta)) = (Self::item_ref(params), text(params, "delta")) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if !self.open.contains_key(&key) {
            self.open.insert(key.clone(), Open::Text);
            out.push(put(
                key.clone(),
                at,
                EntryKind::AgentMessage(AgentMessage {
                    markdown: String::new(),
                    streaming: true,
                }),
            ));
        }
        out.push(Update::Append {
            key,
            text: delta.to_string(),
        });
        out
    }

    fn thought_delta(&mut self, params: &Value, summary: bool, at: SystemTime) -> Vec<Update> {
        let (Some(key), Some(delta)) = (Self::item_ref(params), text(params, "delta")) else {
            return Vec::new();
        };
        let thought = |text: &str| {
            EntryKind::Thought(Thought {
                text: text.to_string(),
                streaming: true,
                ..Thought::default()
            })
        };
        let open = self.open.entry(key.clone()).or_insert_with(|| Open::Thought {
            summary: false,
            raw: false,
            parts: 0,
        });
        let Open::Thought {
            summary: has_summary,
            raw,
            ..
        } = open
        else {
            return Vec::new();
        };
        match (summary, *has_summary, *raw) {
            (true, false, true) => {
                *has_summary = true;
                vec![put(key, at, thought(delta))]
            }
            (true, _, _) | (false, false, _) => {
                let first = !*has_summary && !*raw;
                *has_summary |= summary;
                *raw |= !summary;
                let mut out = Vec::new();
                if first {
                    out.push(put(key.clone(), at, thought("")));
                }
                out.push(Update::Append {
                    key,
                    text: delta.to_string(),
                });
                out
            }
            (false, true, _) => Vec::new(),
        }
    }

    fn thought_part(&mut self, params: &Value) -> Vec<Update> {
        let Some(key) = Self::item_ref(params) else {
            return Vec::new();
        };
        match self.open.get_mut(&key) {
            Some(Open::Thought { parts, .. }) => {
                *parts += 1;
                if *parts > 1 {
                    return vec![Update::Append {
                        key,
                        text: "\n\n".into(),
                    }];
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn output_delta(&mut self, params: &Value) -> Vec<Update> {
        let (Some(key), Some(delta)) = (Self::item_ref(params), text(params, "delta")) else {
            return Vec::new();
        };
        let Some(Open::Tool { output }) = self.open.get_mut(&key) else {
            return Vec::new();
        };
        output.push_str(delta);
        if output.len() > OUTPUT_LIMIT {
            let mut cut = output.len() - OUTPUT_LIMIT;
            while !output.is_char_boundary(cut) {
                cut += 1;
            }
            output.drain(..cut);
        }
        let patch = ToolPatch {
            output: Some(ToolOutput::Terminal {
                stdout: output.clone(),
                stderr: String::new(),
                interrupted: false,
            }),
            ..ToolPatch::default()
        };
        vec![Update::Tool { key, patch }]
    }

    fn patch_updated(&mut self, params: &Value) -> Vec<Update> {
        let Some(key) = Self::item_ref(params).filter(|k| self.open.contains_key(k)) else {
            return Vec::new();
        };
        let changes = params
            .get("changes")
            .and_then(|c| Vec::<FileChange>::deserialize(c).ok())
            .unwrap_or_default();
        let card = cards::file_change(&changes, "inProgress");
        let patch = ToolPatch {
            title: Some(card.title),
            view: Some(card.view),
            output: card.output,
            input: Some(card.input),
            ..ToolPatch::default()
        };
        vec![Update::Tool { key, patch }]
    }

    fn plan(&mut self, params: &Value, at: SystemTime) -> Vec<Update> {
        let list = todos(params.get("plan").unwrap_or(&Value::Null));
        let key = format!("plan:{}", text(params, "turnId").unwrap_or_default());
        vec![Update::Todos(list.clone()), put(key, at, EntryKind::Todos(list))]
    }

    fn error_notification(&mut self, params: &Value, at: SystemTime) -> Vec<Update> {
        let error = params.get("error").unwrap_or(&Value::Null);
        let message = error_message(error);
        if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
            let key = format!("retry:{}", text(params, "turnId").unwrap_or_default());
            let notice = Notice::Info {
                level: InfoLevel::Warning,
                text: message,
            };
            return vec![put(key, at, EntryKind::Notice(notice))];
        }
        self.failed = true;
        self.error(error_kind(error), message, at)
    }

    fn rerouted(&mut self, params: &Value, caps: &mut Capabilities, at: SystemTime) -> (Vec<Update>, bool) {
        let Some(to) = text(params, "toModel") else {
            return (Vec::new(), false);
        };
        caps.model = Some(to.to_string());
        let from = text(params, "fromModel").unwrap_or_default();
        let notice = self.notice(InfoLevel::Notice, format!("{from} → {to}"), at);
        (vec![Update::Model(to.to_string()), notice], true)
    }

    fn warning(&mut self, params: &Value, level: InfoLevel, at: SystemTime) -> Vec<Update> {
        let Some(message) = text(params, "message").or_else(|| text(params, "summary")) else {
            return Vec::new();
        };
        let text = match text(params, "details").filter(|d| !d.is_empty()) {
            Some(details) => format!("{message}\n{details}"),
            None => message.to_string(),
        };
        vec![self.notice(level, text, at)]
    }
}

/// The account after `account/updated`, which names only how Codex is signed in and the plan.
fn account_updated(params: &Value, caps: &mut Capabilities) -> Vec<Update> {
    let email = match &caps.account {
        Some(Account::ChatGpt { email, .. }) => email.clone(),
        _ => None,
    };
    caps.account = match text(params, "authMode") {
        None => None,
        Some("apikey") => Some(Account::ApiKey),
        Some("chatgpt" | "chatgptAuthTokens") => Some(Account::ChatGpt {
            email,
            plan: text(params, "planType").unwrap_or_default().to_string(),
        }),
        Some(other) => Some(Account::Provider(other.to_string())),
    };
    vec![usage::subscription(caps)]
}
