//! What the driver's tests share: a registry whose hop to the main thread is a channel the test thread empties, the
//! signals each subscriber was handed, and the stand-ins of the agents.

#![allow(dead_code, reason = "each test file uses its own part")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use term_agents::{AgentPath, Batch, Outcome, Registry, SessionId, Signal, SubscriberId, TurnEnd, TurnId};
use term_process::Supervisor;

/// The longest a test waits for anything.
pub(crate) const WAIT: Duration = Duration::from_secs(20);

/// A registry and the test thread as its main thread.
pub(crate) struct Harness {
    pub(crate) registry: Registry,
    woken: Receiver<SessionId>,
    /// What each subscriber was handed, batch by batch.
    pub(crate) batches: HashMap<SubscriberId, Vec<Batch>>,
    /// The subscribers the test takes from as soon as they are told; others wait until the test takes.
    pub(crate) eager: Vec<SubscriberId>,
}

impl Harness {
    pub(crate) fn new() -> Harness {
        let (tx, woken) = channel();
        let tx = Mutex::new(tx);
        let wake = Arc::new(move |id: SessionId| {
            let _ = tx.lock().unwrap().send(id);
        });
        Harness {
            registry: Registry::new(Supervisor::new(), wake),
            woken,
            batches: HashMap::new(),
            eager: Vec::new(),
        }
    }

    /// Subscribes to `session` and takes from the subscriber whenever it is told.
    pub(crate) fn follow(&mut self, session: SessionId) -> SubscriberId {
        let driver = self.registry.get_mut(session).expect("the session");
        let subscriber = driver.subscribe();
        let first = driver.take(subscriber).expect("the first batch");
        self.batches.entry(subscriber).or_default().push(first);
        self.eager.push(subscriber);
        subscriber
    }

    /// Takes what waits for `subscriber` of `session` now.
    pub(crate) fn take(&mut self, session: SessionId, subscriber: SubscriberId) -> Option<Batch> {
        let batch = self.registry.get_mut(session)?.take(subscriber)?;
        self.batches.entry(subscriber).or_default().push(batch.clone());
        Some(batch)
    }

    /// Every signal `subscriber` was handed, in order.
    pub(crate) fn signals(&self, subscriber: SubscriberId) -> Vec<Signal> {
        self.batches
            .get(&subscriber)
            .into_iter()
            .flatten()
            .flat_map(|b| b.signals.clone())
            .collect()
    }

    /// Plays the main thread until `done` holds for the signals of `subscriber`.
    pub(crate) fn until(&mut self, subscriber: SubscriberId, what: &str, done: impl Fn(&[Signal]) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.signals(subscriber)) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "waited in vain for {what}: {:?}",
                self.signals(subscriber)
            );
            self.step(left.min(Duration::from_millis(50)));
        }
    }

    /// Plays the main thread until `done` holds for the registry.
    pub(crate) fn until_state(&mut self, what: &str, done: impl Fn(&Registry) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.registry) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "waited in vain for {what}");
            self.step(left.min(Duration::from_millis(50)));
        }
    }

    /// One hop: waits at most `most` for a session to ask, flushes it and takes for the eager subscribers.
    fn step(&mut self, most: Duration) {
        let Ok(session) = self.woken.recv_timeout(most) else {
            return;
        };
        let told = self.registry.flush(session);
        for subscriber in told {
            if self.eager.contains(&subscriber) {
                self.take(session, subscriber);
            }
        }
    }

    /// Waits for the end of `turn` as `subscriber` hears it.
    pub(crate) fn end_of(&mut self, subscriber: SubscriberId, turn: TurnId) -> TurnEnd {
        self.until(subscriber, &format!("the end of {turn}"), |signals| {
            ended(signals, turn).is_some()
        });
        ended(&self.signals(subscriber), turn).unwrap()
    }
}

/// The end of `turn` among `signals`.
pub(crate) fn ended(signals: &[Signal], turn: TurnId) -> Option<TurnEnd> {
    signals.iter().find_map(|s| match s {
        Signal::TurnEnded(end) if end.turn == turn => Some(end.clone()),
        _ => None,
    })
}

/// The request that waits for the user among `signals`, the last one.
pub(crate) fn waiting(signals: &[Signal]) -> Option<String> {
    signals.iter().rev().find_map(|s| match s {
        Signal::Waiting { request, .. } => Some(request.clone()),
        _ => None,
    })
}

/// Whether `outcome` is a failure whose words contain `part`.
pub(crate) fn failed_with(outcome: &Outcome, part: &str) -> bool {
    matches!(outcome, Outcome::Failed(message) if message.contains(part))
}

/// A folder of its own for one test.
pub(crate) fn folder(name: &str) -> term_base::testing::Scratch {
    term_base::testing::scratch(&format!("session-{name}"))
}

/// The stand-in program of the Codex and OpenCode servers.
pub(crate) fn stand_in() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_session-stand-in"))
}

/// The environment of a stand-in `which` that logs what it is sent to `log`.
pub(crate) fn stand_in_env(which: &str, log: &Path) -> Vec<(String, String)> {
    let log_variable = if which == "codex" {
        "CODEX_TEST_LOG"
    } else {
        "OPENCODE_TEST_LOG"
    };
    vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("SESSION_STAND_IN".into(), which.into()),
        (log_variable.into(), log.to_string_lossy().into_owned()),
    ]
}

/// The lines a stand-in logged.
pub(crate) fn logged(log: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// A `claude` in `dir` that answers `initialize`, waits for the prompt and plays `frames`, then, when `after` is
/// given, waits for one more line and plays `after`; it ends after the frames when `exit`. It writes its arguments
/// to `args.txt` and every line it reads after `initialize` to `sent.ndjson` beside it.
pub(crate) fn claude(
    dir: &Path,
    frames: &[String],
    after: &[String],
    exit: bool,
) -> (AgentPath, Vec<(String, String)>) {
    let init = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-1",
        "response": {"commands": [], "models": [{"value": "haiku", "displayName": "Haiku"}],
                     "account": {"tokenSource": "claude.ai", "apiProvider": "firstParty"}, "output_style": "default"}}});
    std::fs::write(dir.join("init.json"), format!("{init}\n")).unwrap();
    std::fs::write(dir.join("frames.ndjson"), frames.join("\n") + "\n").unwrap();
    std::fs::write(dir.join("after.ndjson"), after.join("\n") + "\n").unwrap();
    let sent = dir.join("sent.ndjson");
    let script = dir.join("claude");
    let then = if exit {
        "exit 0".to_string()
    } else {
        format!("cat >> \"{}\"", sent.display())
    };
    let after = if after.is_empty() {
        String::new()
    } else {
        format!(
            "read -r next\nprintf '%s\\n' \"$next\" >> \"{}\"\ncat \"{}\"\n",
            sent.display(),
            dir.join("after.ndjson").display()
        )
    };
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{args}\"\nread -r init\ncat \"{init}\"\nread -r prompt\nprintf '%s\\n' \"$prompt\" >> \"{sent}\"\ncat \"{frames}\"\n{after}{then}\n",
        args = dir.join("args.txt").display(),
        init = dir.join("init.json").display(),
        sent = sent.display(),
        frames = dir.join("frames.ndjson").display(),
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
    (AgentPath::claude(script), env)
}

/// What the `claude` of `claude` in `dir` read after `initialize`.
pub(crate) fn claude_read(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("sent.ndjson")).unwrap_or_default()
}

/// The arguments the `claude` of `claude` in `dir` was started with.
pub(crate) fn claude_args(dir: &Path) -> Vec<String> {
    let args = std::fs::read_to_string(dir.join("args.txt")).unwrap_or_default();
    args.lines().map(str::to_string).collect()
}

/// The frames the CLI printed in the recording `name` of `term-claude`, before any line containing `until`.
pub(crate) fn recorded(name: &str, until: Option<&str>) -> Vec<String> {
    let path = format!("{}/../term-claude/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .take_while(|l| until.is_none_or(|u| !l.contains(u)))
        .filter(|l| !l.starts_with(term_claude::replay::SENT) && !l.contains("control_response"))
        .map(str::to_string)
        .collect()
}
