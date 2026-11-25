//! The supervisor as the `Spawner` the terminal sessions hand their shells to (E3, E21).

use std::time::Duration;

use term_base::{OnStop, ProcessId, Spawner};

use crate::{Kind, Supervisor};

impl Spawner for Supervisor {
    fn register(&self, pid: u32, label: &str) -> ProcessId {
        Supervisor::register(self, pid, Kind::Terminal, label)
    }

    fn on_stop(&self, id: ProcessId, stop: OnStop) {
        Supervisor::on_stop(self, id, stop);
    }

    fn stop(&self, id: ProcessId, grace: Duration) {
        Supervisor::stop(self, id, grace);
    }

    fn forget(&self, id: ProcessId) {
        Supervisor::forget(self, id);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_shell_handed_over_as_a_spawner_is_listed_and_stopped() {
        let supervisor = Supervisor::new();
        let spawner: &dyn Spawner = &supervisor;
        let mut child = Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let id = spawner.register(child.id(), "zsh");
        let running = supervisor.running();
        assert_eq!(
            (running.len(), running[0].kind, running[0].label.as_str()),
            (1, Kind::Terminal, "zsh")
        );
        let hung_up = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&hung_up);
        spawner.on_stop(id, Box::new(move || flag.store(true, Ordering::SeqCst)));
        let waiter = std::thread::spawn(move || child.wait());
        spawner.stop(id, Duration::from_millis(200));
        waiter.join().unwrap().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !supervisor.running().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(hung_up.load(Ordering::SeqCst));
        assert!(supervisor.running().is_empty());
    }

    #[test]
    fn a_forgotten_shell_leaves_the_list() {
        let supervisor = Supervisor::new();
        let id = Spawner::register(&supervisor, std::process::id(), "zsh");
        Spawner::forget(&supervisor, id);
        assert!(supervisor.running().is_empty());
    }
}
