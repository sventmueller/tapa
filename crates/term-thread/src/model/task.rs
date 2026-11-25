//! The subagents a thread started and the work it keeps running in the background.

use std::time::{Duration, SystemTime};

/// Where a subagent or background task stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Not yet running.
    Pending,
    /// Running.
    Running,
    /// Paused.
    Paused,
    /// Finished.
    Completed,
    /// Failed.
    Failed,
    /// Stopped before it finished.
    Stopped,
}

impl TaskStatus {
    /// Whether it has ended.
    pub fn is_final(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped)
    }
}

/// An agent the thread's agent started for part of the work, with a timeline of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Subagent {
    /// The tool call that started it, or its task id when no call is known; its lane is named after this.
    pub key: String,
    /// The agent's task id for it, which stopping it names.
    pub task_id: Option<String>,
    /// Which sort of agent it is.
    pub agent_type: Option<String>,
    /// What it is to do, in a few words.
    pub description: String,
    /// Where it stands.
    pub status: TaskStatus,
    /// When it started.
    pub started: SystemTime,
    /// When it ended.
    pub ended: Option<SystemTime>,
    /// How long it has run, as the agent last reported.
    pub elapsed: Duration,
    /// Tokens it has used.
    pub tokens: u64,
    /// Tools it has called.
    pub tool_uses: u64,
    /// The tool it called last.
    pub last_tool: Option<String>,
    /// The agent's one-line summary of its progress.
    pub summary: Option<String>,
    /// Whether it can be stopped on its own.
    pub cancellable: bool,
    /// Whether it runs in the background.
    pub backgrounded: bool,
    /// The key of the subagent that started it.
    pub parent: Option<String>,
}

/// The sort of work a background item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundKind {
    /// A shell command.
    Shell,
    /// A command watched for output.
    Monitor,
    /// A subagent.
    Subagent,
    /// A workflow.
    Workflow,
    /// Something else, by the agent's name for it.
    Other(String),
}

/// The latest output of a background item, as far as the app has it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutputTail {
    /// The file the agent writes the whole output to.
    pub path: Option<String>,
    /// The end of the output as last fetched.
    pub text: Option<String>,
    /// The size of the whole output in bytes.
    pub total_bytes: Option<u64>,
    /// Whether `text` holds only the end of it.
    pub truncated: bool,
}

/// Work the agent keeps running beside its turn.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundItem {
    /// The agent's task id.
    pub id: String,
    /// What sort of work it is.
    pub kind: BackgroundKind,
    /// What the user reads.
    pub label: String,
    /// Where it stands.
    pub status: TaskStatus,
    /// When it started, when known.
    pub started: Option<SystemTime>,
    /// When it ended, when known.
    pub ended: Option<SystemTime>,
    /// The tool call that started it.
    pub tool_call: Option<String>,
    /// The task id of the subagent that started it.
    pub parent: Option<String>,
    /// Its latest output.
    pub output: OutputTail,
    /// Whether it was moved to or started in the background.
    pub is_backgrounded: bool,
    /// Whether it is housekeeping that activity indicators leave out.
    pub ambient: bool,
}
