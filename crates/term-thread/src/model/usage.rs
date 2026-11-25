//! What a thread costs and uses: tokens, money, the context window and rate limits.

use std::time::{Duration, SystemTime};

use super::EntryId;

/// Tokens, cost and how full the context window is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Input tokens so far.
    pub input_tokens: u64,
    /// Output tokens so far.
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub(crate) cache_write_tokens: u64,
    /// The agent's estimate of what the thread has cost, in US dollars.
    pub cost_usd: Option<f64>,
    /// The session cost in the unit the agent reports, including currencies other than USD and credits.
    pub reported_cost: Option<ReportedCost>,
    /// The latest token report. ACP agents disagree on whether these counts cover a turn or a session, so these
    /// are neither accumulated nor used as context occupancy.
    pub reported_tokens: Option<TokenReport>,
    /// Tokens the context window holds now.
    pub context_used: Option<u64>,
    /// Tokens the context window can hold.
    pub context_size: Option<u64>,
    /// The finished turns, oldest first.
    pub turns: Vec<TurnUsage>,
    /// The session's totals by model, the most expensive first.
    pub models: Vec<ModelCost>,
    /// What fills the context window, as the agent last broke it down; experimental with Claude (E21), so the UI
    /// hides what is `None`.
    pub breakdown: Option<ContextBreakdown>,
    /// Whether the agent runs on a subscription rather than an API key: its use counts against the plan's limits,
    /// and the cost it reports is what the tokens would cost through the API, not what the user pays (E57).
    pub subscription: bool,
}

impl Usage {
    /// The turn that finished last.
    pub fn last_turn(&self) -> Option<&TurnUsage> {
        self.turns.last()
    }

    /// What the thread costs the user so far: nothing to show on a subscription (E57).
    pub fn billed_cost(&self) -> Option<f64> {
        self.cost_usd.filter(|_| !self.subscription)
    }
}

/// A cost reported by an agent, without currency conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportedCost {
    /// The cumulative session amount.
    pub amount: f64,
    /// The currency or other unit supplied by the agent.
    pub currency: String,
}

/// Token counts exactly as the latest agent response reports them. Categories can overlap; `total` is the
/// reported total rather than a sum of the breakdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenReport {
    /// The reported total.
    pub total: u64,
    /// Reported input tokens, without inferring whether cache is included.
    pub input: u64,
    /// Reported output tokens, without inferring whether reasoning is included.
    pub output: u64,
    /// Cache reads, when the agent supplies them.
    pub cache_read: Option<u64>,
    /// Cache writes, when the agent supplies them.
    pub cache_write: Option<u64>,
    /// Reasoning tokens, which may already be included in output.
    pub thought: Option<u64>,
}

/// What one finished turn took.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnUsage {
    /// The user message that started it.
    pub message: Option<EntryId>,
    /// What it cost, in US dollars; `None` when the session's cost before it is not known.
    pub cost_usd: Option<f64>,
    /// The tokens it used, its subagents' included; `None` when the session's tokens before it are not known.
    pub tokens: Option<Tokens>,
    /// How long it ran.
    pub duration: Option<Duration>,
}

/// Token counts of a turn or a model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
}

impl Tokens {
    /// All of them.
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_write
    }
}

/// One model's share of a session.
#[derive(Debug, Clone, Default, PartialEq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct ModelCost {
    /// The model.
    pub model: String,
    /// What it cost, in US dollars.
    pub cost_usd: f64,
    /// The tokens it used.
    pub tokens: Tokens,
    pub context_window: Option<u64>,
}

/// What fills the context window, row by row as the agent's own context view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBreakdown {
    /// The rows, in the agent's order.
    pub rows: Vec<ContextRow>,
    /// Tokens in use.
    pub used: u64,
    /// Tokens the window holds.
    pub size: u64,
    /// The use at which the conversation is compacted by itself, when that is on.
    pub auto_compact_at: Option<u64>,
    /// The model the window belongs to.
    pub model: String,
    /// Whether every row was counted rather than estimated from the last response.
    pub exact: bool,
    /// When the agent answered.
    pub at: SystemTime,
    /// What some rows consist of: memory files, MCP tools, agents, skills, built-in tools and kinds of messages.
    pub parts: Vec<ContextPart>,
}

/// One row of the context breakdown.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct ContextRow {
    /// The agent's stable name for the row, such as `system_prompt`, `messages` or `free_space`, when it sends one.
    pub id: Option<String>,
    /// The row's title as the agent words it.
    pub name: String,
    pub tokens: u64,
    pub kind: ContextKind,
}

/// What a row of the context breakdown is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextKind {
    /// Content in the window.
    Used,
    /// The room left.
    Free,
    /// The reserve kept for compaction.
    Buffer,
    /// Tool definitions loaded only when needed, outside the window.
    Deferred,
}

/// One part of a context row.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct ContextPart {
    /// Which sort of part.
    pub group: ContextGroup,
    /// Its name: a file path, a tool or agent name, or for messages a stable id such as `tool_results`.
    pub name: String,
    pub tokens: u64,
}

/// The sort of a context part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextGroup {
    /// A memory file such as `CLAUDE.md`.
    MemoryFile,
    /// A tool of an MCP server.
    McpTool,
    /// A custom agent.
    Agent,
    /// A skill.
    Skill,
    /// A built-in tool.
    SystemTool,
    /// A kind of message in the conversation.
    Message,
}

/// Whether a plan limit still lets requests through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStatus {
    /// Requests go through.
    Allowed,
    /// Requests go through, but the limit is close.
    Warning,
    /// Requests are refused until the window resets.
    Rejected,
}

/// One usage window of the user's plan, such as five hours or seven days.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimit {
    /// The agent's name for the window, such as `five_hour`.
    pub window: String,
    /// Whether it still lets requests through.
    pub status: LimitStatus,
    /// The share used, from 0 to 1.
    pub utilization: Option<f64>,
    /// When the window starts over.
    pub resets_at: Option<SystemTime>,
}
