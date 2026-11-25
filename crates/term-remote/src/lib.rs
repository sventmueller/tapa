//! Workspaces on another machine (E25): the target a workspace connects to, one master connection per workspace over
//! the system's OpenSSH that every terminal and agent of the workspace shares, the commands that run there whatever
//! the user's shell is, and the helper that answers ssh's questions about passwords and host keys through the app.

pub mod askpass;
mod connection;
pub mod registry;
mod script;
pub mod stand_in;
mod target;

pub use connection::{Connection, Probe, State};
pub use script::{agent_script, through_any_shell};
pub use stand_in::Forward;
pub use target::Target;

/// The OpenSSH client of macOS, which reads the user's `~/.ssh/config`, keys and agent (A6).
pub const SSH: &str = "/usr/bin/ssh";
