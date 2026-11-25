//! Agents over the Agent Client Protocol (E21): a session with an agent such as Antigravity or Gemini CLI that fills
//! the shared thread model, the register of agents the app knows and the ACP registry, and installing them with
//! pinned versions on the user's word; an agent's own command on the login shell's PATH goes first.
//!
//! The protocol is ACP v1 through the `agent-client-protocol` crate; none of its types leave this crate, so raising
//! its version touches nothing else. `docs/acp.md` lays down the version choice and how each update changes a thread.

#![forbid(unsafe_code)]

pub mod capabilities;
mod diff;
pub mod install;
pub mod launch;
pub mod mapping;
pub mod mcp;
mod prompt;
pub mod register;
pub mod sandbox;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod test_agent;
mod tool;
mod usage;

pub use capabilities::{
    AuthMethod, Capabilities, Category, ConfigOption, Mode, OptionChoice, OptionValue, SlashCommand, TerminalSignIn,
};
pub use install::{InstallError, Installed, Installer, Installs, Plan};
pub use launch::Launch;
pub use register::{Distribution, Entry, Preset, Register, Untrusted};
pub use sandbox::{Effect, Sandbox};
pub use session::{Choice, Event, Session, SessionError};
