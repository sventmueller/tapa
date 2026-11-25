//! OpenAI Codex over its own app-server protocol (E147, "SDK first, ACP as fallback"): `codex app-server` on stdio,
//! a session that fills the shared thread model like an ACP session does, and the management calls Codex offers
//! beyond ACP (models, configuration, MCP servers, skills, account, usage limits, feedback).
//!
//! The protocol is the app-server's v2 as Codex 0.167 generates its schema; nothing of it leaves this crate but the
//! thread model's types and the small result types of `manage`. An older Codex is refused with
//! `SessionError::Unsupported`, so the app can use `codex-acp` through `term-acp` instead. `docs/codex.md` lays down
//! the framing, the handshake and how each notification changes a thread.

#![forbid(unsafe_code)]

pub mod capabilities;
mod diff;
pub mod launch;
pub mod manage;
pub mod mapping;
mod prompt;
pub mod protocol;
mod requests;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod test_server;
pub mod version;
pub mod wire;

pub use capabilities::{Account, Capabilities, Effort, MODES, Mode, Model};
pub use launch::{ApprovalPolicy, Launch, SandboxMode};
pub use manage::{
    AccountState, ConfigEdit, ConfigWritten, Feedback, FeedbackSent, Login, LoginStarted, McpLogin, McpServer,
    MergeStrategy, Skill, Skills, ThreadPage, ThreadQuery, ThreadSummary,
};
pub use mapping::Mapper;
pub use session::{Choice, Event, Reply, Session, SessionError};
pub use version::{TESTED, Version};

/// The id of the app-server connection on its threads; the ACP adapter's preset is `codex-acp`.
pub const AGENT: &str = "codex";

/// The parameters this crate sends with `initialize`, for tests that check them against the schema.
pub fn initialize_params() -> serde_json::Value {
    session::initialize_params()
}
