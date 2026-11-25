//! What a thread holds: its entries, the subagents it started and the work it keeps running in the background.

mod message;
mod notice;
mod request;
mod task;
mod tool;
mod usage;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub use message::*;
pub use notice::*;
pub use request::*;
pub use task::*;
pub use tool::*;
pub use usage::*;

/// The app's own name for a thread, stable across restarts and agent sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(String);

impl ThreadId {
    /// A new id that no other thread on this Mac has.
    pub fn new() -> ThreadId {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        ThreadId(format!("t{nanos:x}-{:x}-{n:x}", std::process::id()))
    }

    /// The id as stored.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ThreadId {
    fn default() -> ThreadId {
        ThreadId::new()
    }
}

impl From<&str> for ThreadId {
    fn from(id: &str) -> ThreadId {
        ThreadId(id.to_string())
    }
}

impl std::fmt::Display for ThreadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The number of an entry within its thread; never reused, so the UI can keep a row for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub(crate) u64);

impl EntryId {
    /// The number as such.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Which timeline an entry belongs to: the thread's own or that of one of its subagents.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Lane {
    /// The thread's own timeline.
    Main,
    /// The timeline of the subagent with this key.
    Subagent(String),
}

/// Whether the agent works, waits for the user or rests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Nothing runs in the foreground.
    #[default]
    Idle,
    /// A turn is running.
    Running,
    /// The agent waits for an answer: a permission, a question or a plan.
    RequiresAction,
}

/// What the agent is busy with beyond its turn, as the status line shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// It sends a request to the model.
    Requesting,
    /// It compacts the conversation.
    Compacting,
}

/// One line of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Its number in the thread.
    pub id: EntryId,
    /// When it was added.
    pub at: SystemTime,
    /// What it shows.
    pub kind: EntryKind,
    /// The agent's id for the last frame of the conversation this entry stands for; resuming up to and including
    /// the entry names it. None for what is not part of the conversation, such as notices and subagent entries.
    pub uuid: Option<String>,
    /// The agent's id for the frame of the conversation just before this entry; resuming there drops the entry and
    /// everything after it. None for the conversation's first entry.
    pub follows: Option<String>,
}

/// What an entry shows.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryKind {
    /// What the user sent.
    UserMessage(UserMessage),
    /// The agent's answer as markdown, possibly still streaming.
    AgentMessage(AgentMessage),
    /// The agent's reasoning.
    Thought(Thought),
    /// A tool the agent calls.
    ToolCall(ToolCall),
    /// The agent's to-do list at one point of the turn.
    Todos(Vec<Todo>),
    /// The agent asks to use a tool.
    PermissionRequest(PermissionRequest),
    /// The agent asks the user questions with options.
    Question(Question),
    /// The agent asks to leave plan mode with this plan.
    PlanApproval(PlanApproval),
    /// A status line: compaction, a retry, an error, hook output, a limit.
    Notice(Notice),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_thread_ids_differ_and_a_stored_one_reads_back_unchanged() {
        let (a, b) = (ThreadId::default(), ThreadId::default());
        assert_ne!(a, b);
        let stored = ThreadId::from(a.to_string().as_str());
        assert_eq!(stored.as_str(), a.as_str());
        assert_eq!(stored, a);
        assert_eq!(EntryId(7).get(), 7);
        let tokens = Tokens {
            input: 1,
            output: 2,
            cache_read: 30,
            cache_write: 400,
        };
        assert_eq!(tokens.total(), 433);
    }
}
