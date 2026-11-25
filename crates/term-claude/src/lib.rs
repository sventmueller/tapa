//! The Claude connection (E21): the installed `claude` CLI in stream-json mode, as the official extension drives it,
//! without passing for the extension. Frames are read leniently; what the app does not know is kept raw and
//! ignored, never fatal. `docs/claude-mapping.md` lays down how each frame changes the thread.

#![forbid(unsafe_code)]

pub mod anthropic;
pub mod capabilities;
pub mod cli;
pub mod control;
pub mod history;
pub mod hooks;
pub mod input;
pub mod launch;
pub mod mapping;
pub mod modes;
pub mod protocol;
pub mod replay;
pub mod rules;
pub mod session;
mod tools;

pub use capabilities::{Capabilities, Feature, Tier};
pub use control::{ControlError, PermissionAnswer, PlanAnswer};
pub use launch::{Launch, Thinking};
pub use mapping::Mapper;
pub use modes::{Effort, PermissionMode};
pub use protocol::Message;
pub use session::{Event, RemoteControl, Reply, Session, SessionError};

/// The id of the Claude connection in the agent register and on its threads.
pub const AGENT: &str = "claude";
