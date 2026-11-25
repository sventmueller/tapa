//! Every process the app starts goes through one supervisor: in a process group of its own, listed for the activity
//! view, and ended in order — a graceful step, then SIGTERM, then SIGKILL — when it is stopped or the app quits.
//! [`tool`] finds and runs the short-lived helper programs others need. Signposts, thread priorities, the login shell's
//! environment and the system's process table are in `term_base`.

mod agent_tree;
pub mod framing;
pub mod pipes;
mod priority;
pub mod reaper;
pub mod sandbox;
mod spawner;
pub mod supervisor;
pub mod tail;
pub mod tool;

pub use supervisor::{Ending, Event, Kind, ProcessId, ProcessInfo, Spawned, Subscription, Supervisor};
