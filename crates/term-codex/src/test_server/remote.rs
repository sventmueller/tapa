//! Local stand-in for the Codex daemon and its proxy.

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{Read, Write};

use super::Server;

const SERVER_NAME: &str = "test-mac";

pub(super) fn command(args: &[&str], scenario: &str) -> Option<i32> {
    let home = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from);
    match args {
        ["remote-control", "start", "--json"] => {
            save_remote(home.as_deref(), true);
            println!("{}", json!({"status": "connected", "serverName": SERVER_NAME}));
        }
        ["remote-control", "pair", "--json"] => {
            println!(
                "{}",
                json!({"manualPairingCode": "TEST-1234", "expiresAt": 2_000_000_000u64})
            );
        }
        ["app-server", "daemon", "start"] => {
            println!("{}", json!({"status": "alreadyRunning"}));
        }
        ["app-server", "proxy"] => {
            let mut server = Server::new(std::io::stdout().lock(), scenario);
            server.shared = true;
            server.home = home;
            let log = std::env::var_os("CODEX_TEST_LOG").map(std::path::PathBuf::from);
            server.websocket(std::io::stdin(), log.as_deref());
        }
        _ => return None,
    }
    Some(0)
}

impl<W: Write> Server<W> {
    fn websocket(&mut self, input: impl Read + Send + 'static, log: Option<&std::path::Path>) {
        let mut input = std::io::BufReader::new(Chunks::of(input));
        if crate::ws::read_request(&mut input).is_err() {
            eprintln!("protocol: no WebSocket upgrade");
            return;
        }
        // The real server never answers a connection whose frames came before its answer to the upgrade.
        std::thread::sleep(std::time::Duration::from_millis(50));
        if !input.buffer().is_empty() || input.get_mut().arrived() {
            eprintln!("protocol: frames came before the upgrade was answered");
            return;
        }
        let _ = self.out.write_all(crate::ws::upgrade_answer());
        let _ = self
            .out
            .write_all(&crate::ws::frame(crate::ws::PING, b"stand-in", false));
        let _ = self.out.flush();
        let note = |line: &str| {
            if let Some(path) = log
                && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
            {
                let _ = writeln!(file, "{line}");
            }
        };
        loop {
            let mut pong = |opcode: u8, payload: Vec<u8>| {
                if opcode == crate::ws::PONG {
                    note(&json!({"pong": String::from_utf8_lossy(&payload)}).to_string());
                }
            };
            let message = crate::ws::read_message(&mut input, crate::ws::Peer::Client, &mut pong);
            let text = match message {
                Ok(Some(text)) => text,
                Ok(None) => return,
                Err(error) => {
                    eprintln!("protocol: {error}");
                    return;
                }
            };
            note(&text);
            if let Ok(message) = serde_json::from_str::<Value>(&text) {
                self.handle(&message);
            }
            if self.exit {
                return;
            }
        }
    }

    /// Where Remote Control stands, as the server says it.
    pub(super) fn remote_status(&self) -> Value {
        let status = if self.shared && remote_enabled(self.home.as_deref()) {
            "connected"
        } else {
            "disabled"
        };
        json!({"status": status, "serverName": SERVER_NAME, "installationId": "inst_test", "environmentId": null})
    }

    pub(super) fn disable_remote(&mut self, id: &Value) {
        if !self.experimental {
            return self.error(id, -32600, "remoteControl/disable requires experimentalApi capability");
        }
        save_remote(self.home.as_deref(), false);
        let status = self.remote_status();
        self.reply(id, status.clone());
        self.send(&json!({"method": "remoteControl/status/changed", "params": status}));
    }

    pub(super) fn foreign(&mut self, id: &Value) {
        self.note("thread/started", json!({"thread": {"id": "foreign"}}));
        self.note(
            "turn/started",
            json!({"threadId": "foreign", "turn": {"id": "other", "status": "inProgress"}}),
        );
        self.send(&json!({"id": 994, "method": "item/commandExecution/requestApproval",
                          "params": {"threadId": "foreign", "turnId": "other", "itemId": "foreign_cmd",
                                     "command": "echo foreign", "cwd": "/work", "availableDecisions": ["accept", "decline"]}}));
        self.reply(id, json!({}));
    }
}

struct Chunks {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    rest: VecDeque<u8>,
}

impl Chunks {
    fn of(mut input: impl Read + Send + 'static) -> Chunks {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n @ 1..) = input.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Chunks {
            rx,
            rest: VecDeque::new(),
        }
    }

    /// Whether bytes came that nobody read yet.
    fn arrived(&mut self) -> bool {
        while let Ok(chunk) = self.rx.try_recv() {
            self.rest.extend(chunk);
        }
        !self.rest.is_empty()
    }
}

impl Read for Chunks {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.rest.is_empty() {
            match self.rx.recv() {
                Ok(chunk) => self.rest.extend(chunk),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.rest.len());
        for (slot, byte) in buf.iter_mut().zip(self.rest.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}

/// The file of the daemon's saved choices under `home`.
fn settings(home: &std::path::Path) -> std::path::PathBuf {
    home.join("app-server-daemon/settings.json")
}

fn remote_enabled(home: Option<&std::path::Path>) -> bool {
    home.and_then(|home| std::fs::read(settings(home)).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|s| s["remoteControlEnabled"].as_bool())
        .unwrap_or(false)
}

fn save_remote(home: Option<&std::path::Path>, enabled: bool) {
    if let Some(home) = home {
        let _ = std::fs::create_dir_all(home.join("app-server-daemon"));
        let _ = std::fs::write(settings(home), json!({"remoteControlEnabled": enabled}).to_string());
    }
}
