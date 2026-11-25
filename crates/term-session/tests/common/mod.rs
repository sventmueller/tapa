use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use std::sync::Arc;

use term_base::{Processes, Spawner};
use term_session::{Event, Options, Program, Session};

pub(crate) const PATIENCE: Duration = Duration::from_secs(10);

/// A session with its events collected.
pub(crate) struct Harness {
    pub session: Session,
    pub supervisor: Arc<Processes>,
    pub events: Vec<Event>,
    receiver: Receiver<Event>,
}

impl Harness {
    pub(crate) fn start(options: Options, home: &Path) -> Harness {
        let supervisor = Arc::new(Processes::new());
        let spawner: Arc<dyn Spawner> = supervisor.clone();
        let (sender, receiver) = channel();
        let session = Session::spawn(&spawner, &environment(home), options, move |event| {
            let _ = sender.send(event);
        })
        .expect("the session starts");
        Harness {
            session,
            supervisor,
            events: Vec::new(),
            receiver,
        }
    }

    /// Waits until `done` holds, collecting events; whether it did within `PATIENCE`.
    pub(crate) fn wait_until(&mut self, mut done: impl FnMut(&Harness) -> bool) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            while let Ok(event) = self.receiver.try_recv() {
                self.events.push(event);
            }
            if done(self) {
                return true;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            if let Ok(event) = self.receiver.recv_timeout(left.min(Duration::from_millis(50))) {
                self.events.push(event);
            }
        }
    }

    pub(crate) fn screen_has(&self, text: &str) -> bool {
        self.session.visible_text().iter().any(|line| line.contains(text))
    }

    pub(crate) fn screen_line(&self, text: &str) -> bool {
        self.session.visible_text().iter().any(|line| line.trim_end() == text)
    }

    pub(crate) fn saw(&self, wanted: &Event) -> bool {
        self.events.contains(wanted)
    }

    /// The number after `prefix` on the screen, as printed by `echo prefix$$`.
    pub(crate) fn number_after(&self, prefix: &str) -> Option<i32> {
        self.session.visible_text().iter().find_map(|line| {
            let at = line.find(prefix)? + prefix.len();
            let digits: String = line[at..].chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
    }
}

impl Drop for Harness {
    /// Waits for the program to end, so that it writes nothing more into a test's folder once that is removed.
    fn drop(&mut self) {
        self.session.close();
        let deadline = Instant::now() + PATIENCE;
        while self.session.is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

pub(crate) fn environment(home: &Path) -> Vec<(String, String)> {
    [
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
        ("HOME", &home.to_string_lossy()),
        ("USER", &std::env::var("USER").unwrap_or_default()),
        ("LANG", "en_US.UTF-8"),
        ("BASH_SILENCE_DEPRECATION_WARNING", "1"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

pub(crate) use term_base::testing::scratch as temporary;

pub(crate) fn command(program: &str, args: &[&str], cwd: &Path) -> Options {
    Options {
        program: Program::Command {
            program: program.into(),
            args: args.iter().map(ToString::to_string).collect(),
        },
        shell_integration: None,
        ..Options::new(cwd)
    }
}

/// Whether a process numbered `pid` exists, asked of `kill -0`.
pub(crate) fn alive(pid: i32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn gone_within(pid: i32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}
