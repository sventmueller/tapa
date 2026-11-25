//! Status lines in the timeline: errors, retries, compactions, hooks and rewinds.

use std::time::Duration;

use super::RateLimit;

/// A status line in the timeline.
#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    /// The conversation was compacted.
    Compaction {
        /// Whether the user or the agent started it.
        manual: bool,
        /// Tokens before.
        tokens_before: u64,
        /// Tokens after, when the agent reports them.
        tokens_after: Option<u64>,
    },
    /// A request failed and is tried again.
    Retrying {
        /// The attempt that failed, counted from 1.
        attempt: u32,
        /// How many attempts are made at most.
        max_attempts: u32,
        /// How long the agent waits before the next one.
        delay: Duration,
        /// The agent's name for the error.
        error: String,
        /// The HTTP status, when there was a response.
        http_status: Option<u16>,
    },
    /// Something went wrong.
    Error {
        /// What sort of failure it was.
        kind: ErrorKind,
        /// What the user reads.
        message: String,
    },
    /// A hook ran.
    Hook {
        /// The agent's id for this run of the hook.
        id: String,
        /// The hook's name.
        name: String,
        /// The event it ran for.
        event: String,
        /// What it printed.
        output: String,
        /// Its exit code, once it ended.
        exit_code: Option<i32>,
        /// Where it stands.
        outcome: HookOutcome,
    },
    /// A plan limit is close or reached.
    RateLimit(RateLimit),
    /// The user stopped the turn.
    Interrupted,
    /// The agent refused to rewind the conversation because more than the dropped turn would be lost; the same
    /// rewind fails again, so the session is resumed without it.
    RewindRejected {
        /// The agent's explanation.
        detail: String,
    },
    /// Anything else the agent wants shown.
    Info {
        /// How prominent it is.
        level: InfoLevel,
        /// The text.
        text: String,
    },
}

impl Notice {
    /// The tokens a compaction freed, when it is one and the agent reported the result.
    pub fn freed_tokens(&self) -> Option<u64> {
        match self {
            Notice::Compaction {
                tokens_before,
                tokens_after: Some(after),
                ..
            } => Some(tokens_before.saturating_sub(*after)),
            _ => None,
        }
    }
}

/// What sort of failure an error notice reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The agent is not signed in or its sign-in was refused; the user signs in with the agent's own command.
    Auth,
    /// The model's API failed.
    Api,
    /// The turn ended with an error, such as reaching its turn limit.
    Turn,
    /// The agent's process failed or ended unasked; the message is the end of its error output.
    Process,
    /// Compacting the conversation failed.
    Compaction,
    /// The disk is full, so the agent could not save the conversation; the message is the system's error code.
    Disk,
    /// Anything else.
    Other,
}

/// Where a hook run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// It runs.
    Running,
    /// It succeeded.
    Success,
    /// It failed.
    Error,
    /// It was cancelled.
    Cancelled,
}

/// How prominent an informational notice is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoLevel {
    /// Only for those who look closely.
    Info,
    /// Shown quietly.
    Notice,
    /// Shown as a hint.
    Suggestion,
    /// Shown as a warning.
    Warning,
}

/// Rewinding the conversation to just before a prompt: the agent resumes at `fork_at` as a new branch and checks
/// that this drops only the turn the prompt `drops_turn` started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewind {
    /// The agent's id for the last frame of the turn that is kept.
    pub fork_at: String,
    /// The agent's id for the prompt whose turn is dropped.
    pub drops_turn: String,
}
