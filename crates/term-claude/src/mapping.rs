//! Turns the CLI's frames into thread updates, as `docs/claude-mapping.md` lays down.

mod assistant;
mod context;
mod requests;
mod result;
mod stream;
mod system;
mod tasks;
mod user;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use term_thread::{
    BackgroundPatch, EntryKind, ErrorKind, Lane, Notice, Status, SubagentPatch, TaskStatus, Tokens, ToolPatch,
    ToolStatus, Update, UserMessage,
};

use crate::control::UsageReport;
use crate::protocol::{BackgroundTask, Message, ModelUsage};

pub(crate) use assistant::sign_in_problem;
pub(crate) use result::rewind_rejected;
use system::rate_limit;
pub(crate) use user::image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Text,
    Thought,
}

#[derive(Debug)]
struct Streamed {
    slot: Slot,
    key: String,
    claimed: bool,
}

#[derive(Debug, Default)]
struct Stream {
    message: String,
    input_tokens: u64,
}

#[derive(Debug)]
struct Tool {
    name: String,
    lane: Lane,
}

#[derive(Debug)]
struct Task {
    tool_use_id: Option<String>,
    subagent: Option<String>,
    started: SystemTime,
    output_file: Option<String>,
}

#[derive(Debug)]
struct Ask {
    key: String,
    tool_use_id: String,
}

/// The state that turns a stream of frames into thread updates; one per session.
#[derive(Debug, Default)]
pub struct Mapper {
    history: bool,
    model: Option<String>,
    turn: u64,
    streams: HashMap<Lane, Stream>,
    streamed: HashMap<String, Vec<Streamed>>,
    open: Vec<String>,
    unnamed: u64,
    tools: HashMap<String, Tool>,
    running: HashSet<String>,
    tool_requests: HashMap<String, String>,
    tasks: HashMap<String, Task>,
    runs: HashMap<String, String>,
    level: Vec<BackgroundTask>,
    seen: HashMap<String, SystemTime>,
    live_subagents: HashSet<String>,
    asks: HashMap<String, Ask>,
    /// The plans the user answered in this session, by their tool call.
    answered: HashSet<String>,
    frames: HashMap<String, Vec<String>>,
    auth_reported: bool,
    chain: Vec<String>,
    /// The session's totals after the last turn, which the next turn's figures are the difference to.
    totals: Totals,
    /// Whether `totals` are not known, as when the CLI resumed a session and may count from an earlier process.
    totals_unknown: bool,
    /// The prompts sent so far.
    prompts: u64,
    /// The ids of the prompts the app sent, whose echoes it leaves out.
    sent: HashSet<String>,
    /// Whether the last status sent was other than idle.
    busy: bool,
}

/// What the CLI counts for the whole session: its cost and tokens over all models.
#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    cost_usd: f64,
    tokens: Tokens,
}

fn tokens_of<'a>(models: impl Iterator<Item = &'a ModelUsage>) -> Tokens {
    models.fold(Tokens::default(), |sum, m| Tokens {
        input: sum.input + m.input_tokens,
        output: sum.output + m.output_tokens,
        cache_read: sum.cache_read + m.cache_read_input_tokens,
        cache_write: sum.cache_write + m.cache_creation_input_tokens,
    })
}

fn tokens_since(now: Tokens, before: Tokens) -> Tokens {
    Tokens {
        input: now.input.saturating_sub(before.input),
        output: now.output.saturating_sub(before.output),
        cache_read: now.cache_read.saturating_sub(before.cache_read),
        cache_write: now.cache_write.saturating_sub(before.cache_write),
    }
}

fn lane(parent: Option<&String>) -> Lane {
    parent.map_or(Lane::Main, |p| Lane::Subagent(p.clone()))
}

/// Whether `message` is the agent answering in the main conversation rather than in a subagent's.
fn writes_in_main(message: &Message) -> bool {
    match message {
        Message::Assistant(a) => a.parent_tool_use_id.is_none(),
        Message::StreamEvent(e) => e.parent_tool_use_id.is_none(),
        _ => false,
    }
}

fn put(lane: &Lane, key: Option<String>, at: SystemTime, entry: EntryKind) -> Update {
    Update::Put {
        lane: lane.clone(),
        key,
        at,
        entry,
    }
}

fn notice(at: SystemTime, notice: Notice) -> Update {
    put(&Lane::Main, None, at, EntryKind::Notice(notice))
}

fn seconds(secs: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(secs).ok()
}

fn epoch_millis(ms: f64) -> Option<SystemTime> {
    seconds(ms / 1000.0).map(|d| UNIX_EPOCH + d)
}

impl Mapper {
    /// A mapper for a live session, which adds the user's messages itself when they are sent.
    pub fn new() -> Mapper {
        Mapper::default()
    }

    /// A mapper for a saved transcript, which takes the user's messages from the frames.
    pub(crate) fn for_history() -> Mapper {
        Mapper {
            history: true,
            ..Mapper::default()
        }
    }

    /// The updates `message` makes, as of `now`.
    pub fn map(&mut self, message: &Message, now: SystemTime) -> Vec<Update> {
        // The CLI starts a turn of its own when a background task ends, with no frame that says so.
        let mut out = if !self.history && !self.busy && writes_in_main(message) {
            vec![self.status(Status::Running)]
        } else {
            Vec::new()
        };
        out.extend(self.frame(message, now));
        out
    }

    fn status(&mut self, status: Status) -> Update {
        self.busy = status != Status::Idle;
        Update::Status(status)
    }

    fn frame(&mut self, message: &Message, now: SystemTime) -> Vec<Update> {
        match message {
            Message::Assistant(a) => self.assistant(a, now),
            Message::User(u) => self.user(u, now),
            Message::Result(r) => self.result(r, now),
            Message::System(s) => self.system(s, now),
            Message::StreamEvent(e) => self.stream(e, now),
            Message::ToolProgress(p) => {
                let elapsed = seconds(p.elapsed_time_seconds);
                vec![Update::Tool {
                    key: p.tool_use_id.clone(),
                    patch: ToolPatch {
                        status: Some(ToolStatus::InProgress),
                        started: elapsed.and_then(|e| now.checked_sub(e)),
                        elapsed,
                        ..ToolPatch::default()
                    },
                }]
            }
            Message::ToolUseSummary(s) => s
                .preceding_tool_use_ids
                .iter()
                .map(|id| Update::Tool {
                    key: id.clone(),
                    patch: ToolPatch {
                        summary: Some(s.summary.clone()),
                        ..ToolPatch::default()
                    },
                })
                .collect(),
            Message::RateLimit(info) => rate_limit(info, now),
            Message::AuthStatus(a) => a
                .error
                .iter()
                .map(|error| {
                    notice(
                        now,
                        Notice::Error {
                            kind: ErrorKind::Auth,
                            message: error.clone(),
                        },
                    )
                })
                .collect(),
            Message::ConversationReset(_) => self.reset(now),
            Message::PromptSuggestion(_)
            | Message::KeepAlive
            | Message::ControlRequest { .. }
            | Message::ControlResponse(_)
            | Message::ControlCancel { .. }
            | Message::Unknown(_)
            | Message::Malformed(_) => Vec::new(),
        }
    }

    /// The updates for a message the user sends, with `uuid` as the CLI's id for it.
    pub(crate) fn user_message(&mut self, message: UserMessage, uuid: &str, now: SystemTime) -> Vec<Update> {
        self.auth_reported = false;
        self.prompts += 1;
        self.sent.insert(uuid.to_string());
        let key = format!("user:{uuid}");
        let mut out = vec![put(
            &Lane::Main,
            Some(key.clone()),
            now,
            EntryKind::UserMessage(UserMessage {
                message_id: Some(uuid.to_string()),
                ..message
            }),
        )];
        out.extend(self.link(&[key], uuid));
        out.push(self.status(Status::Running));
        out
    }

    /// Appends the main conversation's frame `uuid` to the chain and ties the entries with `keys` to it.
    fn link(&mut self, keys: &[String], uuid: &str) -> Vec<Update> {
        let follows = self.chain.last().cloned();
        self.chain.push(uuid.to_string());
        keys.iter()
            .map(|key| Update::Chain {
                key: key.clone(),
                uuid: uuid.to_string(),
                follows: follows.clone(),
            })
            .collect()
    }

    /// Forgets the session's totals: a resumed CLI may count on from an earlier process, so the next turn's cost
    /// stays unknown unless `baseline` learns the totals first.
    pub(crate) fn resumed(&mut self) {
        self.totals_unknown = true;
    }

    /// The prompts sent so far.
    pub(crate) fn prompts(&self) -> u64 {
        self.prompts
    }

    /// Takes the session's totals from a `get_usage` answer that was asked for after `prompts` prompts, unless they
    /// are known already or a prompt was sent since, whose turn the answer may count in part.
    pub(crate) fn baseline(&mut self, report: &UsageReport, prompts: u64) {
        if !self.totals_unknown || self.prompts != prompts {
            return;
        }
        if let Some(cost_usd) = report.session_cost_usd {
            self.totals = Totals {
                cost_usd,
                tokens: tokens_of(report.session_models.values()),
            };
            self.totals_unknown = false;
        }
    }

    /// The updates when the CLI's process has ended: nothing streams, waits or runs any more.
    pub fn ended(&mut self, now: SystemTime) -> Vec<Update> {
        let mut out = self.close_streams(now);
        let asks: Vec<String> = self.asks.keys().cloned().collect();
        for request in asks {
            out.extend(self.withdrawn(&request));
        }
        for call in self.running.drain() {
            out.push(Update::Tool {
                key: call,
                patch: ToolPatch {
                    status: Some(ToolStatus::Failed),
                    ended: Some(now),
                    ..ToolPatch::default()
                },
            });
        }
        for key in self.live_subagents.drain() {
            let mut patch = SubagentPatch::new(key, now);
            patch.status = Some(TaskStatus::Stopped);
            patch.ended = Some(now);
            patch.cancellable = Some(false);
            out.push(Update::Subagent(patch));
        }
        for task in self.level.drain(..) {
            out.push(Update::BackgroundItem(BackgroundPatch {
                id: task.task_id,
                status: Some(TaskStatus::Stopped),
                ended: Some(now),
                ..BackgroundPatch::default()
            }));
        }
        out.push(Update::Background(Vec::new()));
        out.push(Update::Activity(None));
        out.push(self.status(Status::Idle));
        out
    }

    fn close_streams(&mut self, now: SystemTime) -> Vec<Update> {
        self.streamed.clear();
        self.open
            .drain(..)
            .map(|key| Update::Finish {
                key,
                text: None,
                at: now,
            })
            .collect()
    }

    fn reset(&mut self, now: SystemTime) -> Vec<Update> {
        self.streams.clear();
        self.streamed.clear();
        self.open.clear();
        self.tools.clear();
        self.running.clear();
        self.frames.clear();
        self.chain.clear();
        vec![Update::Clear, Update::Background(self.background_items(now))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_texts(updates: &[Update]) -> Vec<String> {
        updates
            .iter()
            .filter_map(|u| match u {
                Update::Put {
                    entry: EntryKind::UserMessage(m),
                    ..
                } => Some(m.text.clone()),
                _ => None,
            })
            .collect()
    }

    fn echo(mapper: &mut Mapper, frame: &str) -> Vec<String> {
        let message = crate::protocol::parse(frame).unwrap();
        user_texts(&mapper.map(&message, SystemTime::UNIX_EPOCH))
    }

    #[test]
    fn only_echoes_of_prompts_typed_elsewhere_become_user_messages() {
        let mut mapper = Mapper::new();
        let own = UserMessage {
            text: "from the app".into(),
            ..UserMessage::default()
        };
        assert_eq!(
            user_texts(&mapper.user_message(own, "u-1", SystemTime::UNIX_EPOCH)),
            ["from the app"]
        );
        let replay = |uuid: &str, extra: &str, content: &str| {
            format!(
                r#"{{"type":"user","uuid":"{uuid}","isReplay":true,{extra}"message":{{"role":"user","content":"{content}"}}}}"#
            )
        };
        assert!(echo(&mut mapper, &replay("u-1", "", "from the app")).is_empty());
        assert!(echo(&mut mapper, &replay("s-1", r#""isSynthetic":true,"#, "made up")).is_empty());
        assert!(echo(&mut mapper, &replay("c-1", "", "<bash-stdout>ls</bash-stdout>")).is_empty());
        let task = replay("t-1", r#""origin":{"kind":"task-notification"},"#, "done");
        assert!(echo(&mut mapper, &task).is_empty());
        let phone = replay("p-1", r#""origin":{"kind":"human"},"#, "from the phone");
        assert_eq!(echo(&mut mapper, &phone), ["from the phone"]);
        assert_eq!(mapper.prompts(), 2);
        let mut history = Mapper::for_history();
        assert!(echo(&mut history, &replay("p-2", "", "from the phone")).is_empty());
    }

    #[test]
    fn background_work_still_running_when_the_cli_ends_is_stopped_then() {
        let mut mapper = Mapper::new();
        let set = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","task_type":"local_bash","description":"loop"}],"session_id":"s","uuid":"u"}"#;
        mapper.map(&crate::protocol::parse(set).unwrap(), SystemTime::UNIX_EPOCH);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(7);
        let updates = mapper.ended(at);
        let stopped = updates.iter().position(|u| {
            matches!(u, Update::BackgroundItem(BackgroundPatch { id, status: Some(TaskStatus::Stopped), ended: Some(when), .. })
                if id == "b1" && *when == at)
        });
        let emptied = updates
            .iter()
            .position(|u| matches!(u, Update::Background(items) if items.is_empty()));
        assert!(stopped.zip(emptied).is_some_and(|(s, e)| s < e), "{updates:?}");
    }

    #[test]
    fn a_background_agent_ends_with_the_notification_the_transcript_keeps() {
        let frames = [
            r#"{"type":"assistant","message":{"id":"m1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_bg","name":"Agent","input":{"description":"review code","subagent_type":"general-purpose","prompt":"Review","run_in_background":true}}]},"session_id":"s","uuid":"a1"}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bg","content":"Async agent launched successfully."}]},"session_id":"s","uuid":"u1"}"#,
            r#"{"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>a1b2</task-id>\n<tool-use-id>toolu_bg</tool-use-id>\n<status>completed</status>\n<summary>Agent \"review code\" finished</summary>\n<result>It quotes <status>failed</status> and <task-id>x</task-id>.</result>\n<usage><subagent_tokens>1234</subagent_tokens><tool_uses>7</tool_uses><duration_ms>65000</duration_ms></usage>\n</task-notification>"},"session_id":"s","uuid":"u2"}"#,
        ];
        let mut mapper = Mapper::for_history();
        let mut thread = term_thread::Thread::new(term_thread::ThreadId::new(), "claude", "/");
        for frame in frames {
            let message = crate::protocol::parse(frame).unwrap();
            thread.apply_all(mapper.map(&message, SystemTime::UNIX_EPOCH));
        }
        let agent = thread.subagent("toolu_bg").expect("the agent call starts a subagent");
        assert_eq!(agent.status, TaskStatus::Completed);
        assert_eq!((agent.tokens, agent.tool_uses), (1234, 7));
        assert_eq!(agent.elapsed, Duration::from_secs(65));
        let users = thread
            .entries(&Lane::Main)
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::UserMessage(_)))
            .count();
        assert_eq!(users, 0, "the notification is no message of the user");
    }

    fn statuses(mapper: &mut Mapper, frame: &str) -> Vec<Status> {
        let message = crate::protocol::parse(frame).unwrap();
        mapper
            .map(&message, SystemTime::UNIX_EPOCH)
            .into_iter()
            .filter_map(|u| match u {
                Update::Status(status) => Some(status),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn background_agents_running_after_the_turn_are_no_turn() {
        let state = |state: &str| {
            format!(
                r#"{{"type":"system","subtype":"session_state_changed","state":"{state}","session_id":"s","uuid":"u"}}"#
            )
        };
        let result = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s"}"#;
        let mut mapper = Mapper::new();
        mapper.user_message(UserMessage::default(), "u-1", SystemTime::UNIX_EPOCH);
        assert_eq!(
            statuses(&mut mapper, &state("requires_action")),
            [Status::RequiresAction]
        );
        assert_eq!(statuses(&mut mapper, &state("running")), [Status::Running]);
        assert_eq!(statuses(&mut mapper, result), [Status::Idle]);
        assert!(statuses(&mut mapper, &state("running")).is_empty());
        assert_eq!(statuses(&mut mapper, &state("idle")), [Status::Idle]);
    }

    #[test]
    fn a_turn_the_cli_starts_after_a_background_task_runs() {
        let start = |parent: &str| {
            format!(
                r#"{{"type":"stream_event","event":{{"type":"message_start","message":{{"id":"m","type":"message","role":"assistant","content":[]}}}},"parent_tool_use_id":{parent},"session_id":"s","uuid":"u"}}"#
            )
        };
        let result = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s"}"#;
        let mut mapper = Mapper::new();
        mapper.user_message(UserMessage::default(), "u-1", SystemTime::UNIX_EPOCH);
        assert!(statuses(&mut mapper, &start("null")).is_empty());
        assert_eq!(statuses(&mut mapper, result), [Status::Idle]);
        assert!(statuses(&mut mapper, &start(r#""toolu_bg""#)).is_empty());
        assert_eq!(statuses(&mut mapper, &start("null")), [Status::Running]);
        assert!(statuses(&mut mapper, &start("null")).is_empty());
        assert_eq!(statuses(&mut mapper, result), [Status::Idle]);
        let mut history = Mapper::for_history();
        assert!(statuses(&mut history, &start("null")).is_empty());
    }
}
