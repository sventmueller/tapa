//! The gate of E3: a session started by a host that is not this app, with an identity and a spawner of its own, as
//! Dango will start one after the migration (P7). It prints what the shell printed and what the host's spawner saw.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use term_base::{Identity, OnStop, ProcessId, Spawner};
use term_session::{Event, Options, Program, Session};

/// The host's own register of processes, standing in for Dango's supervisor.
#[derive(Default)]
struct HostSupervisor {
    next: AtomicUsize,
    seen: Mutex<Vec<String>>,
}

impl Spawner for HostSupervisor {
    fn register(&self, pid: u32, label: &str) -> ProcessId {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{label} ({pid})"));
        ProcessId(u64::try_from(self.next.fetch_add(1, Ordering::Relaxed)).unwrap_or(0))
    }

    fn on_stop(&self, _id: ProcessId, _stop: OnStop) {}

    fn stop(&self, _id: ProcessId, _grace: Duration) {}

    fn forget(&self, id: ProcessId) {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("forgot {}", id.0));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    term_base::set_identity(Identity {
        name: "HostApp".into(),
        version: "9.9".into(),
        bundle_id: "example.host".into(),
    });
    let supervisor = Arc::new(HostSupervisor::default());
    let spawner: Arc<dyn Spawner> = supervisor.clone();
    let mut options = Options::new(std::env::temp_dir());
    options.program = Program::Command {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "echo \"$TERM_PROGRAM $TERM_PROGRAM_VERSION\"".into()],
    };
    options.shell_integration = None;
    let (tx, rx) = channel();
    let session = Session::spawn(&spawner, &[], options, move |event| {
        let _ = tx.send(event);
    })?;
    while !matches!(rx.recv_timeout(Duration::from_secs(10))?, Event::Exited(_)) {}
    let printed = session.visible_text()[0].trim_end().to_string();
    println!("shell printed: {printed}");
    println!(
        "host spawner saw: {:?}",
        supervisor
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    );
    println!("data folder: {}", term_base::data_dir().display());
    assert_eq!(printed, "HostApp 9.9");
    assert!(term_base::data_dir().ends_with("example.host"));
    Ok(())
}
