//! A small stand-in for `opencode serve --stdio` in tests: it serves the part of OpenCode 2.2.6's API this crate uses
//! on a free port of 127.0.0.1, prints the same address line, refuses every request without the basic auth of the
//! password in `OPENCODE_SERVER_PASSWORD` (401, as the real server does), ends when its input closes, and plays fixed
//! turns with every kind of event and request the mapper reads. Its answers and events follow the schema in
//! `tests/fixtures`, which the crate's tests check them against.
//!
//! The scenario comes from `OPENCODE_TEST_SCENARIO`: `basic`, `nomodel` (no provider has a model), `locked` (a
//! password nobody knows), `old` (an unsupported release, which only `--version` shows). A prompt containing `slow`
//! streams one delta and waits for the interrupt; `crash` says [`CRASH`] on stderr and exits mid-turn; `ask` asks a
//! form; `fail` ends the turn with an error; `gap` streams one text, then ends every event stream and plays the end
//! of the turn while none is open, after which the session's info and messages show the turn as done
//! ([`GAP_ANSWER`]); any other plays a turn with reasoning, text, to-dos, a read, a command
//! that asks for permission, an edit, usage and a new title. With `OPENCODE_TEST_LOG` set, every request is appended
//! to that file as one JSON line with its method, path, query, body and whether it was authorized.

mod data;
mod turn;
mod wire;

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};

use turn::Step;
use wire::Request;

/// The id of the session the server creates.
pub const SESSION: &str = "ses_test";

/// The id of the session a fork creates.
pub const FORK: &str = "ses_fork";

/// The id of the permission the turn asks for.
pub const PERMISSION: &str = "per_test1";

/// The id of the form the `ask` turn shows.
pub const FORM: &str = "frm_test1";

/// The output of the command the user allowed.
pub const COMMAND_OUTPUT: &str = "a.txt\nb.txt";

/// What the server writes on stderr before it exits mid-turn.
pub const CRASH: &str = "the model crashed";

/// The answer of the turn whose end a gap in the event stream swallowed, as the session's messages keep it.
pub const GAP_ANSWER: &str = "Working, and done.";

/// What `--version` prints, unless the scenario is `old`.
pub const VERSION: &str = "opencode v2.2.6";

/// What `--version` prints in the scenario `old`.
pub const OLD_VERSION: &str = "opencode v2.1.0";

/// The variable that names the scenario.
pub const SCENARIO_VARIABLE: &str = "OPENCODE_TEST_SCENARIO";

/// The variable that names the file every request is logged to.
pub const LOG_VARIABLE: &str = "OPENCODE_TEST_LOG";

/// The time of the first event, in milliseconds since 1970; each later one is 100 ms after the one before.
const CLOCK: u64 = 1_700_000_100_000;

/// The events without a `durable` member in the schema.
const VOLATILE: [&str; 13] = [
    "session.text.delta",
    "session.reasoning.delta",
    "session.tool.input.delta",
    "session.tool.progress",
    "session.compaction.delta",
    "session.usage.updated",
    "session.idle",
    "permission.asked",
    "permission.replied",
    "form.created",
    "form.replied",
    "form.cancelled",
    "tui.toast.show",
];

/// The durable events at version 2 of their schema; every other durable one is at version 1.
const VERSION_TWO: [&str; 5] = [
    "session.tool.success",
    "session.tool.failed",
    "session.deleted",
    "session.forked",
    "session.instructions.updated",
];

/// What `opencode --version` prints in `scenario`.
pub fn version(scenario: &str) -> &'static str {
    if scenario == "old" { OLD_VERSION } else { VERSION }
}

/// What the turn waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    Permission,
    Form,
    Interrupt,
}

/// Where playing the steps stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Played {
    /// The steps ran out or one waits.
    Rests,
    /// The server is to crash.
    Crash,
    /// The event streams are to break off before the rest is played.
    Gap,
}

struct World {
    scenario: String,
    directory: String,
    events: Vec<String>,
    count: u64,
    steps: VecDeque<Step>,
    waiting: Option<Wait>,
    agent: String,
    model: Value,
    /// Counts the gaps; an event stream ends when it changes.
    generation: u64,
    /// The messages of a turn whose end a gap swallowed, which the session's history then lists.
    recorded: Option<Value>,
}

struct Hub {
    world: Mutex<World>,
    changed: Condvar,
    closed: AtomicBool,
    authorization: Option<String>,
    log: Option<PathBuf>,
    address: SocketAddr,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl World {
    fn location(&self) -> Value {
        json!({"directory": self.directory})
    }

    fn emit(&mut self, kind: &str, data: Value) {
        self.count += 1;
        let n = self.count;
        let mut event = json!({"id": format!("evt_{n:04}"), "created": CLOCK + n * 100, "type": kind,
                               "location": self.location(), "data": data});
        if !VOLATILE.contains(&kind) {
            let version = if VERSION_TWO.contains(&kind) { 2 } else { 1 };
            event["durable"] = json!({"aggregateID": SESSION, "seq": n, "version": version});
        }
        self.events
            .push(format!("id: evt_{n:04}\nevent: {kind}\ndata: {event}\n\n"));
    }

    /// Plays the steps until one waits, the server is to crash or the streams are to break off.
    fn play(&mut self) -> Played {
        while let Some(step) = self.steps.pop_front() {
            match step {
                Step::Event(kind, data) => self.emit(kind, data),
                Step::Permission(data) => {
                    self.emit("permission.asked", data);
                    self.waiting = Some(Wait::Permission);
                    return Played::Rests;
                }
                Step::Form(form) => {
                    self.emit("form.created", json!({"form": form}));
                    self.waiting = Some(Wait::Form);
                    return Played::Rests;
                }
                Step::Hang => {
                    self.waiting = Some(Wait::Interrupt);
                    return Played::Rests;
                }
                Step::Crash => return Played::Crash,
                Step::Gap => return Played::Gap,
                Step::Record(messages) => self.recorded = Some(messages),
            }
        }
        Played::Rests
    }

    /// Goes on after the turn waited for `wait`; false when it did not.
    fn resume(&mut self, wait: Wait, steps: VecDeque<Step>) -> bool {
        if self.waiting != Some(wait) {
            return false;
        }
        self.waiting = None;
        self.steps = steps;
        self.play();
        true
    }
}

impl Hub {
    fn world(&self) -> MutexGuard<'_, World> {
        lock(&self.world)
    }

    fn log(&self, request: &Request, authorized: bool) {
        let Some(path) = &self.log else { return };
        let query: serde_json::Map<String, Value> = request.query.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let line = json!({"method": request.method, "path": request.path, "query": query,
                          "body": request.body, "authorized": authorized});
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            drop(writeln!(file, "{line}"));
        }
    }

    /// Ends the server: the accept loop and every event stream stop.
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.changed.notify_all();
        drop(TcpStream::connect_timeout(&self.address, Duration::from_secs(1)));
    }

    fn serve(self: &Arc<Self>, stream: &TcpStream) {
        let Ok(request) = wire::read(stream) else { return };
        let authorized = self.authorization.is_some() && request.authorization == self.authorization;
        self.log(&request, authorized);
        if !authorized {
            let body = json!({"_tag": "UnauthorizedError", "message": "Unauthorized"});
            drop(wire::respond(stream, 401, Some(&body), false));
            return;
        }
        if request.method == "GET" && request.path == "/api/event" {
            return self.stream(stream);
        }
        let (status, body, chunked) = self.route(&request);
        drop(wire::respond(stream, status, body.as_ref(), chunked));
    }

    fn stream(&self, stream: &TcpStream) {
        if wire::begin_events(stream).is_err() {
            return;
        }
        let connected = json!({"id": "evt_0000", "type": "server.connected", "data": {}});
        if wire::chunk(stream, &format!("event: server.connected\ndata: {connected}\n\n")).is_err() {
            return;
        }
        let mut world = self.world();
        let mut sent = world.events.len();
        let generation = world.generation;
        while !self.closed.load(Ordering::SeqCst) {
            while sent < world.events.len() {
                if world.generation != generation {
                    return;
                }
                let frame = world.events[sent].clone();
                sent += 1;
                drop(world);
                if wire::chunk(stream, &frame).is_err() {
                    return;
                }
                world = self.world();
            }
            if world.generation != generation {
                return;
            }
            world = self
                .changed
                .wait_timeout(world, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn not_found(what: &str) -> (u16, Option<Value>, bool) {
        (
            404,
            Some(json!({"_tag": "NotFoundError", "message": format!("{what} not found")})),
            false,
        )
    }

    /// The answer to `request`: its status, its body and whether it goes out chunked.
    fn route(self: &Arc<Self>, request: &Request) -> (u16, Option<Value>, bool) {
        let segments = request.segments();
        let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
        let method = request.method.as_str();
        let (location, scenario, directory) = {
            let world = self.world();
            (world.location(), world.scenario.clone(), world.directory.clone())
        };
        let ok = |body: Value| (200, Some(body), false);
        if let Some(answer) = self.recorded(method, &parts, &directory) {
            return ok(answer);
        }
        match (method, parts.as_slice()) {
            ("GET", ["info"]) => ok(json!({"version": "2.2.6", "pid": std::process::id(),
                "urls": [format!("http://{}", self.address)], "paths": {"tmp": "/tmp"}})),
            ("GET", ["agent"]) => ok(data::agents(&location)),
            ("GET", ["model"]) => (200, Some(data::models(&location, &scenario)), true),
            ("GET", ["model", "default"]) => ok(data::default_model(&location, &scenario)),
            ("GET", ["provider"]) => ok(data::providers(&location)),
            ("GET", ["mcp"]) => ok(data::mcp(&location)),
            ("PUT" | "DELETE", ["experimental", "mcp", _]) | ("POST", ["experimental", "mcp", _, _]) => {
                (204, None, false)
            }
            ("GET", ["credential"]) => ok(data::credentials()),
            ("GET", ["experimental", "usage", "limits"]) => ok(data::limits(&location)),
            ("GET", ["permission", "request"]) => ok(data::waiting(&location, "ses_old")),
            ("POST", ["session"]) => {
                let world = self.world();
                let info = data::session(SESSION, &directory, "New session", &world.agent, &world.model);
                ok(json!({"data": info}))
            }
            ("GET", ["session"]) => {
                let world = self.world();
                let info = data::session(SESSION, &directory, "Listing files", &world.agent, &world.model);
                ok(json!({"data": [info], "cursor": {"previous": null, "next": null}}))
            }
            ("GET", ["session", "ses_missing"]) => (
                404,
                Some(json!({"_tag": "SessionNotFoundError",
                "sessionID": "ses_missing", "message": "Session not found: ses_missing"})),
                false,
            ),
            ("GET", ["session", id]) => {
                let info = data::session(
                    id,
                    &directory,
                    "Earlier",
                    "plan",
                    &json!({"providerID": "openai", "id": "gpt-5", "variant": "high"}),
                );
                ok(json!({"data": info}))
            }
            ("GET", ["session", _, "message"]) => (200, Some(data::messages(request.query("cursor"))), true),
            ("GET", ["session", _, "form"]) => ok(json!({"data": []})),
            ("POST", ["session", _, "prompt"]) => self.prompt(request),
            ("POST", ["session", _, "interrupt"]) => {
                let interrupted = self.world().resume(Wait::Interrupt, turn::interrupted());
                self.changed.notify_all();
                ok(json!({"interrupted": interrupted}))
            }
            ("POST", ["session", _, "permission", _, "reply"]) => {
                let reply = request.body["decision"].as_str().unwrap_or("reject").to_string();
                self.world().resume(Wait::Permission, turn::permitted(&reply));
                self.changed.notify_all();
                (204, None, false)
            }
            ("POST", ["session", _, "form", _, "reply"]) => {
                self.world()
                    .resume(Wait::Form, turn::answered(Some(&request.body["answer"])));
                self.changed.notify_all();
                (204, None, false)
            }
            ("DELETE", ["session", _, "form", _]) => {
                self.world().resume(Wait::Form, turn::answered(None));
                self.changed.notify_all();
                (204, None, false)
            }
            ("POST", ["session", _, "agent" | "model"]) => self.switch(request, parts[2]),
            ("POST", ["session", _, "fork"]) => ok(data::fork(&directory)),
            ("GET", ["experimental", "session", id, "export"]) => {
                let info = data::session(
                    id,
                    &directory,
                    "Listing files",
                    "build",
                    &json!({"providerID": "anthropic", "id": "claude-sonnet-4"}),
                );
                ok(json!({"data": {"info": info, "messages": data::messages(None)["data"]}}))
            }
            ("POST", ["session", _, "revert", "stage"]) => ok(
                json!({"data": {"messageID": request.body["messageID"], "files": [{"file": "a.txt",
                    "patch": "@@ -1 +1 @@\n-2\n+two\n", "additions": 1, "deletions": 1, "status": "modified"}]}}),
            ),
            ("POST", ["session", _, "revert", "commit"]) | ("DELETE", ["session", _, "revert"]) => (204, None, false),
            _ => Self::not_found(&request.path),
        }
    }

    fn prompt(self: &Arc<Self>, request: &Request) -> (u16, Option<Value>, bool) {
        let text = request.body["text"].as_str().unwrap_or_default().to_string();
        let steer = request.body["delivery"] == "steer";
        let user = json!({"data": {"id": "msg_u1", "sessionID": SESSION, "time": {"created": CLOCK},
            "type": "user", "payload": {"text": text},
            "delivery": if steer { "steer" } else { "queue" }}});
        let played = {
            let mut world = self.world();
            if steer || world.waiting.is_some() || !world.steps.is_empty() {
                return (200, Some(user), false);
            }
            world.steps = turn::turn(&text);
            world.play()
        };
        self.changed.notify_all();
        match played {
            Played::Crash => self.in_a_moment(|hub| {
                eprintln!("{CRASH}");
                hub.close();
            }),
            Played::Gap => self.in_a_moment(Hub::gap),
            Played::Rests => {}
        }
        (200, Some(user), false)
    }

    /// Runs `then` after the streams had time to send what came before, or at once when no thread starts.
    fn in_a_moment(self: &Arc<Self>, then: fn(&Hub)) {
        let hub = self.clone();
        let spawned = std::thread::Builder::new()
            .name("opencode-test-later".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                then(&hub);
            });
        if spawned.is_err() {
            then(self);
        }
    }

    /// Ends every event stream and plays the rest of the turn while none is open, so the client misses it.
    fn gap(&self) {
        {
            let mut world = self.world();
            world.generation += 1;
            world.play();
        }
        self.changed.notify_all();
    }

    /// The session's info and messages once a gap swallowed the end of its turn, as the server keeps them.
    fn recorded(&self, method: &str, parts: &[&str], directory: &str) -> Option<Value> {
        let world = self.world();
        let messages = world.recorded.clone()?;
        match (method, parts) {
            ("GET", ["session", id]) if *id == SESSION => {
                let mut info = data::session(SESSION, directory, "After a gap", &world.agent, &world.model);
                info["time"]["busy"] = json!(CLOCK);
                info["time"]["idle"] = json!(CLOCK + 10_000);
                info["outcome"] = json!("succeeded");
                info["cost"] = json!(0.02);
                info["tokens"] = json!({"input": 300, "output": 40, "reasoning": 0, "cache": {"read": 0, "write": 0}});
                Some(json!({"data": info}))
            }
            ("GET", ["session", id, "message"]) if *id == SESSION => {
                Some(json!({"data": messages, "cursor": {"previous": null, "next": null}}))
            }
            _ => None,
        }
    }

    fn switch(&self, request: &Request, what: &str) -> (u16, Option<Value>, bool) {
        let mut world = self.world();
        if what == "agent" {
            let agent = request.body["agent"].as_str().unwrap_or_default().to_string();
            let previous = std::mem::replace(&mut world.agent, agent.clone());
            world.emit(
                "session.agent.selected",
                json!({"sessionID": SESSION, "agent": agent, "previous": previous}),
            );
        } else {
            let model = request.body["model"].clone();
            let previous = std::mem::replace(&mut world.model, model.clone());
            world.emit(
                "session.model.selected",
                json!({"sessionID": SESSION, "model": model, "previous": previous}),
            );
        }
        drop(world);
        self.changed.notify_all();
        (204, None, false)
    }
}

fn accept(hub: &Arc<Hub>, listener: &TcpListener) {
    for stream in listener.incoming() {
        if hub.closed.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let mine = hub.clone();
        let spawned = std::thread::Builder::new()
            .name("opencode-test-conn".into())
            .spawn(move || mine.serve(&stream));
        if let Err(error) = spawned {
            eprintln!("opencode-test-server: {error}");
        }
    }
}

/// Serves `scenario` until `input` ends, as the real server runs until its standard input closes, or a turn
/// crashes it; the address line goes to `output`. The password is the one in `OPENCODE_SERVER_PASSWORD`, as for the
/// real server, and the working directory is the project folder.
pub fn run(input: impl Read + Send + 'static, mut output: impl Write, scenario: &str, log: Option<PathBuf>) {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(listener) => listener,
        Err(error) => return eprintln!("opencode-test-server: {error}"),
    };
    let Ok(address) = listener.local_addr() else { return };
    let password = std::env::var(crate::launch::PASSWORD_VARIABLE).ok();
    let password = if scenario == "locked" {
        Some("nobody knows".into())
    } else {
        password
    };
    let directory = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let hub = Arc::new(Hub {
        world: Mutex::new(World {
            scenario: scenario.to_string(),
            directory,
            events: Vec::new(),
            count: 0,
            steps: VecDeque::new(),
            waiting: None,
            agent: "build".into(),
            model: json!({"providerID": "anthropic", "id": "claude-sonnet-4"}),
            generation: 0,
            recorded: None,
        }),
        changed: Condvar::new(),
        closed: AtomicBool::new(false),
        authorization: password.map(|p| crate::http::basic(&p)),
        log,
        address,
    });
    let watcher = hub.clone();
    let watching = std::thread::Builder::new()
        .name("opencode-test-stdin".into())
        .spawn(move || {
            let mut input = input;
            let mut sink = [0u8; 256];
            while matches!(input.read(&mut sink), Ok(n) if n > 0) {}
            watcher.close();
        });
    if let Err(error) = watching {
        return eprintln!("opencode-test-server: {error}");
    }
    if writeln!(output, "{}", json!({"url": format!("http://{address}")}))
        .and_then(|()| output.flush())
        .is_err()
    {
        return;
    }
    accept(&hub, &listener);
}
