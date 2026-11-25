//! What every terminal crate shares and nothing of it knows a window: the product's identity and its folders (E3), the
//! `Spawner` that keeps child processes, threads, lanes and timers, cancelling, signposts, the login shell's
//! environment, atomic writes, and the main-thread hops of AppKit callbacks.

pub mod atomic_write;
mod cancel;
pub mod diag;
pub mod explain;
mod identity;
pub mod lane;
pub mod main_guard;
pub mod main_thread;
#[cfg(any(test, feature = "testing"))]
pub mod perf;
pub mod priority;
pub mod redact;
pub mod shell_env;
pub mod signpost;
pub mod spawner;
mod sync;
pub mod system;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod thread;
pub mod timer;
pub mod unattended;

pub use cancel::Cancel;
pub use identity::{Identity, cache_dir, data_dir, home, identity, logs_dir, set_home, set_identity};
pub use main_thread::{guarded, main_after, main_soon};
pub use spawner::{Ending, OnStop, ProcessId, Processes, Spawner};
pub use sync::{IgnoringPoison, LockIgnoringPoison};
