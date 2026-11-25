//! What a session needs of whoever keeps the app's child processes (E3): the app brings a lean register of its own,
//! Dango later its supervisor, so after the migration there are not two of them.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::sync::LockIgnoringPoison;

/// A process in a spawner's register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId(pub u64);

/// How a process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It exited with this code.
    Code(i32),
    /// A signal ended it.
    Signal(i32),
    /// Its owner reported it gone, or waiting for it failed.
    Unknown,
}

/// What ending a process does first, such as hanging up its terminal.
pub type OnStop = Box<dyn FnOnce() + Send>;

/// Keeps the processes a session started, so the app can end them all when it quits.
pub trait Spawner: Send + Sync {
    /// Registers process `pid`, which leads a process group of its own, under `label`.
    fn register(&self, pid: u32, label: &str) -> ProcessId;
    /// Makes `stop` what ending `id` does before the signals that follow.
    fn on_stop(&self, id: ProcessId, stop: OnStop);
    /// Ends `id`: its `on_stop`, then SIGTERM and after `grace` SIGKILL to its group.
    fn stop(&self, id: ProcessId, grace: Duration);
    /// Removes `id`, which has ended.
    fn forget(&self, id: ProcessId);
}

/// The app's own register: a table of process groups and what ends them.
#[derive(Default)]
pub struct Processes {
    next: AtomicU64,
    running: Mutex<HashMap<ProcessId, Entry>>,
}

struct Entry {
    pid: u32,
    on_stop: Option<OnStop>,
}

impl Processes {
    /// An empty register.
    pub fn new() -> Processes {
        Processes::default()
    }

    /// How many processes are registered.
    pub fn len(&self) -> usize {
        self.running.lock_ignoring_poison().len()
    }

    /// Whether no process is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ends every registered process, as quitting the app does.
    pub fn stop_all(&self, grace: Duration) {
        let ids: Vec<ProcessId> = self.running.lock_ignoring_poison().keys().copied().collect();
        for id in ids {
            self.stop(id, grace);
        }
    }
}

impl Spawner for Processes {
    fn register(&self, pid: u32, _label: &str) -> ProcessId {
        let id = ProcessId(self.next.fetch_add(1, Ordering::Relaxed));
        self.running
            .lock_ignoring_poison()
            .insert(id, Entry { pid, on_stop: None });
        id
    }

    fn on_stop(&self, id: ProcessId, stop: OnStop) {
        if let Some(entry) = self.running.lock_ignoring_poison().get_mut(&id) {
            entry.on_stop = Some(stop);
        }
    }

    fn stop(&self, id: ProcessId, grace: Duration) {
        let Some(entry) = self.running.lock_ignoring_poison().remove(&id) else {
            return;
        };
        if let Some(stop) = entry.on_stop {
            stop();
        }
        let group = i32::try_from(entry.pid).unwrap_or(0);
        crate::thread::detach_or_run("process-stop", move || {
            for signal in [libc::SIGTERM, libc::SIGKILL] {
                if crate::system::group_ends_within(group, grace) {
                    return;
                }
                crate::system::signal_group(group, signal);
            }
        });
    }

    fn forget(&self, id: ProcessId) {
        self.running.lock_ignoring_poison().remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn stopping_runs_the_hook_once_and_forgets_the_process() {
        let processes = Processes::new();
        let id = processes.register(0, "nothing");
        let ran = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&ran);
        processes.on_stop(id, Box::new(move || flag.store(true, Ordering::SeqCst)));
        processes.stop(id, Duration::ZERO);
        assert!(ran.load(Ordering::SeqCst));
        assert!(processes.is_empty());
        processes.stop(id, Duration::ZERO);
    }

    #[test]
    fn a_forgotten_process_is_not_stopped() {
        let processes = Processes::new();
        let id = processes.register(0, "gone");
        processes.forget(id);
        assert!(processes.is_empty());
    }
}
