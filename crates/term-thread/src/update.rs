//! The changes an agent connection makes to a thread, and what the UI learns from each.

use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::model::{
    Activity, BackgroundItem, ContextBreakdown, EntryId, EntryKind, Lane, ModelCost, OutputTail, RateLimit,
    RequestState, Status, TaskStatus, Todo, ToolOutput, ToolStatus, ToolView, TurnUsage,
};

/// One change to a thread; entries are addressed by the key the connection gave them.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// The agent's own id for the conversation, which resuming names.
    Session(String),
    /// The thread's title.
    Title(String),
    /// Whether the agent works, waits or rests.
    Status(Status),
    /// What the agent is busy with beyond its turn.
    Activity(Option<Activity>),
    /// The agent's mode, such as its permission mode.
    Mode(String),
    /// The model.
    Model(String),
    /// The reasoning effort.
    Effort(Option<String>),
    /// New totals for tokens, cost or context.
    Usage(UsagePatch),
    /// The state of one usage window; replaces the window of the same name.
    RateLimit(RateLimit),
    /// The agent's current to-do list.
    Todos(Vec<Todo>),
    /// Adds an entry to `lane`, or replaces the content of the entry that already has `key`.
    Put {
        /// The timeline.
        lane: Lane,
        /// The connection's name for the entry, for later updates.
        key: Option<String>,
        /// When it happened.
        at: SystemTime,
        /// What it shows.
        entry: EntryKind,
    },
    /// Appends streamed text to the agent message or thought with `key`.
    Append {
        /// The entry.
        key: String,
        /// The text that arrived.
        text: String,
    },
    /// Ends the streaming of the agent message or thought with `key`.
    Finish {
        /// The entry.
        key: String,
        /// The complete text, which replaces what was streamed.
        text: Option<String>,
        /// When it ended, for a thought's duration.
        at: SystemTime,
    },
    /// Changes the tool call with `key`.
    Tool {
        /// The entry.
        key: String,
        /// What changes.
        patch: ToolPatch,
    },
    /// Changes the state of the permission request, question or plan approval with `key`.
    Resolve {
        /// The entry.
        key: String,
        /// The new state.
        state: RequestState,
    },
    /// Attaches the agent's request id to the question or plan approval with `key`, which then waits for the user.
    Ask {
        /// The entry.
        key: String,
        /// The agent's id for the request.
        request_id: String,
    },
    /// Records which frames of the agent's conversation the entry with `key` stands for: `uuid` replaces the last
    /// one, `follows` is kept from the first time it is given.
    Chain {
        /// The entry.
        key: String,
        /// The agent's id for the entry's latest frame.
        uuid: String,
        /// The agent's id for the frame before the entry's first one.
        follows: Option<String>,
    },
    /// Removes the entry with `key`, such as a reply the agent took back.
    Remove {
        /// The entry.
        key: String,
    },
    /// Adds a subagent or changes the one with the patch's key.
    Subagent(SubagentPatch),
    /// The complete set of background items that run; known items not in it have ended and stay as such.
    Background(Vec<BackgroundItem>),
    /// Changes one background item, if it is there.
    BackgroundItem(BackgroundPatch),
    /// Starts the conversation afresh: entries, subagents, background items, to-dos and usage are cleared.
    Clear,
}

/// New values for some usage fields; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsagePatch {
    /// Input tokens so far.
    pub input_tokens: Option<u64>,
    /// Output tokens so far.
    pub output_tokens: Option<u64>,
    /// Input tokens read from the prompt cache.
    pub cache_read_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    pub cache_write_tokens: Option<u64>,
    /// The cost so far, in US dollars.
    pub cost_usd: Option<f64>,
    /// The cumulative cost in the agent's original currency or unit.
    pub reported_cost: Option<crate::ReportedCost>,
    /// The latest agent token report, without inferring its scope.
    pub reported_tokens: Option<crate::TokenReport>,
    /// Tokens the context window holds now.
    pub context_used: Option<u64>,
    /// Tokens the context window can hold.
    pub context_size: Option<u64>,
    /// A turn that finished; the thread fills in its user message when the patch leaves it out.
    pub turn: Option<TurnUsage>,
    /// The session's totals by model.
    pub models: Option<Vec<ModelCost>>,
    /// A new breakdown of the context window.
    pub breakdown: Option<ContextBreakdown>,
    /// Whether the agent runs on a subscription rather than an API key.
    pub subscription: Option<bool>,
}

/// New values for some fields of a tool call; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolPatch {
    /// The complete input.
    pub input: Option<Value>,
    /// The card's head.
    pub title: Option<String>,
    /// The input field by field.
    pub view: Option<ToolView>,
    /// Where it stands.
    pub status: Option<ToolStatus>,
    /// What it returned.
    pub output: Option<ToolOutput>,
    /// When it began to run; kept once set.
    pub started: Option<SystemTime>,
    /// When it finished; also sets the elapsed time from the start.
    pub ended: Option<SystemTime>,
    /// How long it has run.
    pub elapsed: Option<Duration>,
    /// The number of results of a search.
    pub results: Option<u64>,
    /// The summary of the group of calls it belongs to.
    pub summary: Option<String>,
    /// The background task it became.
    pub background_task: Option<String>,
}

/// New values for some fields of a subagent; `None` leaves a field as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentPatch {
    /// The subagent's key: the tool call that started it, or its task id.
    pub key: String,
    /// When it started; used when the subagent is new.
    pub at: SystemTime,
    /// The agent's task id for it.
    pub task_id: Option<String>,
    /// Which sort of agent it is.
    pub agent_type: Option<String>,
    /// What it is to do.
    pub description: Option<String>,
    /// Where it stands.
    pub status: Option<TaskStatus>,
    /// When it ended.
    pub ended: Option<SystemTime>,
    /// How long it has run.
    pub elapsed: Option<Duration>,
    /// Tokens it has used.
    pub tokens: Option<u64>,
    /// Tools it has called.
    pub tool_uses: Option<u64>,
    /// The tool it called last.
    pub last_tool: Option<String>,
    /// The summary of its progress.
    pub summary: Option<String>,
    /// Whether it can be stopped on its own.
    pub cancellable: Option<bool>,
    /// Whether it runs in the background.
    pub backgrounded: Option<bool>,
    /// The key of the subagent that started it.
    pub parent: Option<String>,
}

impl SubagentPatch {
    /// A patch that changes nothing but names the subagent.
    pub fn new(key: impl Into<String>, at: SystemTime) -> SubagentPatch {
        SubagentPatch {
            key: key.into(),
            at,
            task_id: None,
            agent_type: None,
            description: None,
            status: None,
            ended: None,
            elapsed: None,
            tokens: None,
            tool_uses: None,
            last_tool: None,
            summary: None,
            cancellable: None,
            backgrounded: None,
            parent: None,
        }
    }
}

/// New values for some fields of a background item; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackgroundPatch {
    /// The item's task id.
    pub id: String,
    /// Where it stands.
    pub status: Option<TaskStatus>,
    /// When it ended.
    pub ended: Option<SystemTime>,
    /// Its latest output; fields that are `None` keep what was known.
    pub output: Option<OutputTail>,
    /// Whether it runs in the background.
    pub is_backgrounded: Option<bool>,
}

/// What an update changed, for the UI to redraw just that.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Change {
    /// A field of the thread's head changed.
    Header(Field),
    /// Tokens, cost, context or limits changed.
    Usage,
    /// The to-do list changed.
    Todos,
    /// An entry was added at `index` of `lane`.
    Inserted {
        /// The timeline.
        lane: Lane,
        /// The entry.
        id: EntryId,
        /// Its position in the timeline.
        index: usize,
    },
    /// An entry changed in place.
    Updated {
        /// The timeline.
        lane: Lane,
        /// The entry.
        id: EntryId,
    },
    /// An entry was removed.
    Removed {
        /// The timeline.
        lane: Lane,
        /// The entry.
        id: EntryId,
    },
    /// The subagent with this key was added or changed.
    Subagent(String),
    /// The background items changed.
    Background,
    /// Everything was cleared.
    Cleared,
}

/// A field of the thread's head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// The agent's session id.
    Session,
    /// The title.
    Title,
    /// The status.
    Status,
    /// The activity beyond the turn.
    Activity,
    /// The mode.
    Mode,
    /// The model.
    Model,
    /// The effort.
    Effort,
}
