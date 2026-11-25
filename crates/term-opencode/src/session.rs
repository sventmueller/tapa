//! A running `opencode serve --stdio`: the process the supervisor started, its address and password, the requests
//! the session sends, the event stream it reads and the thread the events fill.
//!
//! Requests and the stream run on threads of their own; the methods only start a request or take a short lock, so
//! the UI never waits for OpenCode.

mod answer;
mod events;
mod manage;
mod open;
mod reply;
mod resync;

use std::collections::HashMap;
use std::io::BufReader;
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use term_process::framing::{self, Line};
use term_process::tail::{self, Tail};
use term_process::{Kind, ProcessId, Supervisor};
use term_thread::{Change, ErrorKind, RequestState, Status, Thread, Update, UserMessage};

use crate::capabilities::Capabilities;
use crate::http::{Endpoint, HttpError, Response, encode};
use crate::launch::{Launch, Secret};
use crate::mapping::Mapper;
use crate::requests::{AskKind, Offer};
use crate::version::Version;

pub use reply::Reply;
pub(crate) use reply::{raw, unit};

/// What the stderr tail keeps.
const STDERR_LIMIT: usize = 64 * 1024;

/// The longest line of the server's standard output read; a longer one is dropped.
const STDOUT_LINE_LIMIT: usize = 64 * 1024;

/// How long the end of a session waits for the stderr of a server that exited on its own.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// What a session tells the UI, on one of the session's threads.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The thread changed; read it with `Session::with_thread`.
    Changed(Vec<Change>),
    /// What the server or the session offers changed: agents, models, the session's agent or model.
    Capabilities,
    /// Something keeps the session from working, such as no connected provider or a release too old.
    Failed(SessionError),
    /// The MCP server of this name changed its state; `Session::mcp_servers` tells the new one.
    McpChanged(String),
    /// The event stream broke off while the server runs; what happened until it is open again is lost, so the
    /// thread may lag behind the server until `Session::resync`.
    StreamGap,
    /// The event stream is open again after a gap; the events in between stay lost.
    StreamResumed,
    /// The process ended; its last error output is attached.
    Ended {
        /// The end of what the server wrote to stderr.
        stderr: String,
    },
}

/// An answer of the server with an error status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    /// The HTTP status.
    pub status: u16,
    /// The error's `_tag`, such as `SessionNotFoundError`.
    pub tag: Option<String>,
    /// What the server says.
    pub message: String,
}

impl ServerError {
    pub(crate) fn from_response(response: &Response) -> ServerError {
        let body = response.json().unwrap_or(Value::Null);
        let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
        ServerError {
            status: response.status,
            tag: text("_tag"),
            message: text("message").unwrap_or_else(|| String::from_utf8_lossy(&response.body).trim().to_string()),
        }
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.tag, self.message.is_empty()) {
            (_, false) => write!(f, "{} ({})", self.message, self.status),
            (Some(tag), true) => write!(f, "{tag} ({})", self.status),
            (None, true) => write!(f, "HTTP {}", self.status),
        }
    }
}

/// Why a session cannot do what was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    /// The program is not there.
    NotFound(PathBuf),
    /// The process did not start or named no address.
    Start(String),
    /// The installed OpenCode is older than this build was tested against, or its version could not be read; the
    /// UI falls back to `opencode acp`.
    Unsupported {
        /// What `opencode --version` named.
        found: Option<Version>,
        /// The oldest release tested.
        tested: Version,
    },
    /// No provider offers a model, so no turn can run before one is connected.
    SignIn,
    /// The request did not reach the server or its answer was not HTTP.
    Http(HttpError),
    /// The server answered with an error.
    Server(ServerError),
    /// The server's answer has another shape than the schema describes.
    Malformed(String),
    /// No request with this id waits for this kind of answer.
    UnknownRequest(String),
    /// The session has no setting, agent or option with this id.
    UnknownOption(String),
    /// A turn is being started; a message can follow once it runs.
    Busy,
    /// No answer came in time.
    Timeout,
    /// The session has ended or has not opened yet.
    Closed,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(path) => write!(f, "{} is not there", path.display()),
            SessionError::Start(why) => write!(f, "opencode did not start: {why}"),
            SessionError::Unsupported {
                found: Some(found),
                tested,
            } => write!(f, "opencode {found} is older than {tested}"),
            SessionError::Unsupported { found: None, .. } => f.write_str("the version of opencode is unknown"),
            SessionError::SignIn => f.write_str("opencode has no provider with a model"),
            SessionError::Http(error) => write!(f, "{error}"),
            SessionError::Server(error) => write!(f, "{error}"),
            SessionError::Malformed(why) => write!(f, "opencode answered unexpectedly: {why}"),
            SessionError::UnknownRequest(id) => write!(f, "no request {id} waits for this answer"),
            SessionError::UnknownOption(id) => write!(f, "opencode has no setting {id}"),
            SessionError::Busy => f.write_str("a turn is starting"),
            SessionError::Timeout => f.write_str("opencode did not answer in time"),
            SessionError::Closed => f.write_str("the session has ended"),
        }
    }
}

impl std::error::Error for SessionError {}

/// The user's answer to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The option at this place in the request's list.
    Option(usize),
    /// No option: a permission is rejected, a form cancelled.
    Dismiss,
}

type Sink = Box<dyn Fn(Event) + Send + Sync>;
type After = Box<dyn FnOnce(&Result<Value, SessionError>, &Arc<Shared>) + Send>;
/// What a request's thread hands its answer to: the `After` and the reply's sender, taken once.
type Handoff = Arc<Mutex<Option<(Option<After>, Sender<Result<Value, SessionError>>)>>>;

struct State {
    mapper: Mapper,
    thread: Arc<Thread>,
    caps: Capabilities,
    /// OpenCode's id of the session, once it is open.
    session_id: Option<String>,
    /// The requests that wait for the user, by OpenCode's id.
    asks: HashMap<String, Offer>,
    /// A prompt went out and has no answer yet.
    starting: bool,
    /// The user stopped a turn the server had not begun yet.
    interrupt: bool,
}

struct Shared {
    supervisor: Supervisor,
    process: ProcessId,
    launch: Launch,
    password: Secret,
    endpoint: Mutex<Option<Endpoint>>,
    stdin: Mutex<Option<ChildStdin>>,
    stream: Mutex<Option<TcpStream>>,
    state: Mutex<State>,
    sink: Sink,
    stderr: Arc<Mutex<Tail>>,
    stderr_reader: Mutex<Option<tail::Collecting>>,
    ended: AtomicBool,
    stopping: AtomicBool,
}

/// A running OpenCode session; clones share it. It runs until `stop`, the supervisor ends it or the server exits.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The address a line of the server's standard output names, `{"url":"http://127.0.0.1:<port>"}`.
fn address_line(line: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(line.trim_ascii()).ok()?;
    value.get("url").and_then(Value::as_str).map(str::to_string)
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn emit(&self, event: Event) {
        (self.sink)(event);
    }

    fn fail(&self, error: SessionError) {
        self.emit(Event::Failed(error));
    }

    fn apply(&self, updates: Vec<Update>) {
        if updates.is_empty() {
            return;
        }
        let changes = Arc::make_mut(&mut self.state().thread).apply_all(updates);
        if !changes.is_empty() {
            self.emit(Event::Changed(changes));
        }
    }

    fn is_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    fn endpoint(&self) -> Result<Endpoint, SessionError> {
        lock(&self.endpoint).clone().ok_or(SessionError::Closed)
    }

    /// The `location` query of the project folder, which the location-scoped operations take.
    fn location(&self) -> String {
        format!("location%5Bdirectory%5D={}", encode(&self.launch.cwd.to_string_lossy()))
    }

    /// The path of the session's own operation `rest`, such as `/prompt`.
    fn session_path(&self, rest: &str) -> Result<String, SessionError> {
        if self.is_ended() {
            return Err(SessionError::Closed);
        }
        let id = self.state().session_id.clone().ok_or(SessionError::Closed)?;
        Ok(format!("/api/session/{}{rest}", encode(&id)))
    }

    /// Sends a request on the calling thread and waits for its answer.
    fn request_now(&self, method: &str, target: &str, body: Option<&Value>) -> Result<Value, SessionError> {
        if self.is_ended() {
            return Err(SessionError::Closed);
        }
        let response = self
            .endpoint()?
            .request(method, target, body)
            .map_err(SessionError::Http)?;
        if !response.is_success() {
            return Err(SessionError::Server(ServerError::from_response(&response)));
        }
        response.json().map_err(|e| SessionError::Malformed(e.to_string()))
    }

    /// Sends a request on a thread of its own; `after` sees the answer there before the reply does.
    fn call<T>(
        self: &Arc<Self>,
        method: &'static str,
        target: String,
        body: Option<Value>,
        after: Option<After>,
        read: reply::Read<T>,
    ) -> Reply<T> {
        let (tx, reply) = Reply::channel(read);
        let slot: Handoff = Arc::new(Mutex::new(Some((after, tx))));
        let theirs = slot.clone();
        let shared = self.clone();
        let spawned = std::thread::Builder::new().name("opencode-call".into()).spawn(move || {
            let result = shared.request_now(method, &target, body.as_ref());
            if let Some((after, tx)) = lock(&theirs).take() {
                if let Some(after) = after {
                    after(&result, &shared);
                }
                drop(tx.send(result));
            }
        });
        if let Err(error) = spawned
            && let Some((after, tx)) = lock(&slot).take()
        {
            let result = Err(SessionError::Start(error.to_string()));
            if let Some(after) = after {
                after(&result, self);
            }
            drop(tx.send(result));
        }
        reply
    }

    /// Asks the server to stop the session's turn.
    fn interrupt(self: &Arc<Self>) {
        if let Ok(path) = self.session_path("/interrupt") {
            drop(self.call("POST", path, None, None, unit));
        }
    }

    /// Answers every open request as refused and withdraws its entry.
    fn withdraw_asks(self: &Arc<Self>) {
        let asks: Vec<(String, Offer)> = self.state().asks.drain().collect();
        let mut updates = Vec::new();
        for (id, offer) in asks {
            let answer = match offer.kind {
                AskKind::Permission => self
                    .session_path(&format!("/permission/{}/reply", encode(&id)))
                    .map(|p| ("POST", p, Some(json!({"decision": "reject"})))),
                AskKind::Form(_) => self
                    .session_path(&format!("/form/{}", encode(&id)))
                    .map(|p| ("DELETE", p, None)),
            };
            if let Ok((method, path, body)) = answer {
                drop(self.call(method, path, body, None, unit));
            }
            updates.push(Update::Resolve {
                key: offer.key,
                state: RequestState::Withdrawn,
            });
        }
        self.apply(updates);
    }

    /// Closes the server's input, which ends it, and the event stream.
    fn close(&self) {
        drop(lock(&self.stdin).take());
        if let Some(stream) = lock(&self.stream).take() {
            drop(stream.shutdown(Shutdown::Both));
        }
    }

    /// Gives the stderr reader a moment to take in what the server wrote just before it exited.
    fn await_stderr(&self) {
        if let Some(reader) = lock(&self.stderr_reader).take() {
            reader.wait(LAST_WORDS);
        }
    }

    /// Reads the server's standard output: the line with its address, then whatever else until the process ends.
    fn read_stdout(self: Arc<Self>, stdout: ChildStdout, url: Sender<String>) {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let mut url = Some(url);
        loop {
            match framing::read_line(&mut reader, &mut line, STDOUT_LINE_LIMIT) {
                Ok(Line::End) | Err(_) => break,
                Ok(Line::TooLong) => {}
                Ok(Line::Read) => {
                    if url.is_some()
                        && let Some(address) = address_line(&line)
                        && let Some(tx) = url.take()
                    {
                        drop(tx.send(address));
                    }
                }
            }
        }
        drop(url);
        self.finish();
    }

    /// The server is gone or was stopped.
    fn finish(&self) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.close();
        let withdrawn: Vec<Update> = self
            .state()
            .asks
            .drain()
            .map(|(_, offer)| Update::Resolve {
                key: offer.key,
                state: RequestState::Withdrawn,
            })
            .collect();
        self.apply(withdrawn);
        let stopping = self.stopping.load(Ordering::SeqCst);
        if !stopping {
            self.await_stderr();
        }
        let stderr = lock(&self.stderr).text();
        if self.state().thread.status() != Status::Idle {
            let updates = {
                let mut state = self.state();
                let mut updates = Vec::new();
                if !stopping {
                    let last: Vec<&str> = stderr.trim().lines().rev().take(20).collect();
                    let message = last.into_iter().rev().collect::<Vec<_>>().join("\n");
                    updates.extend(state.mapper.error(ErrorKind::Process, message, now()));
                }
                updates.extend(state.mapper.abandon(now()));
                updates
            };
            self.apply(updates);
        }
        self.supervisor.stop(self.process, Duration::from_millis(500));
        self.emit(Event::Ended { stderr });
    }
}

impl Session {
    /// Starts `opencode serve` as `launch` describes, filling `thread`; the version check, the handshake and opening
    /// the session follow on a thread of their own.
    ///
    /// ```no_run
    /// use std::sync::mpsc::channel;
    ///
    /// use term_opencode::{Event, Launch, Session};
    /// use term_process::Supervisor;
    /// use term_thread::{Thread, ThreadId};
    ///
    /// let env: Vec<(String, String)> = std::env::vars().collect();
    /// let launch = Launch::new("/opt/homebrew/bin/opencode", env, "/work");
    /// let thread = Thread::new(ThreadId::from("t1"), term_opencode::AGENT, "/work");
    /// let (tx, rx) = channel();
    /// let session = Session::start(Supervisor::global(), launch, thread, move |event| drop(tx.send(event)))?;
    /// while !session.is_ready() {
    ///     if let Event::Failed(error) = rx.recv()? {
    ///         return Err(error.into());
    ///     }
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start(
        supervisor: &Supervisor,
        launch: Launch,
        thread: Thread,
        sink: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Session, SessionError> {
        if !launch.program.is_file() {
            return Err(SessionError::NotFound(launch.program.clone()));
        }
        let password = Secret::generate().map_err(|e| SessionError::Start(e.to_string()))?;
        let spawned = supervisor
            .spawn(launch.command(&password), Kind::Agent, launch.name.clone())
            .map_err(|e| SessionError::Start(e.to_string()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (spawned.stdin, spawned.stdout, spawned.stderr) else {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start("the pipes are missing".into()));
        };
        let origin = u64::try_from(now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()).unwrap_or_default();
        let stderr_tail = Arc::new(Mutex::new(Tail::new(STDERR_LIMIT)));
        let shared = Arc::new(Shared {
            supervisor: supervisor.clone(),
            process: spawned.id,
            launch,
            password,
            endpoint: Mutex::new(None),
            stdin: Mutex::new(Some(stdin)),
            stream: Mutex::new(None),
            state: Mutex::new(State {
                mapper: Mapper::new(origin),
                thread: Arc::new(thread),
                caps: Capabilities::default(),
                session_id: None,
                asks: HashMap::new(),
                starting: false,
                interrupt: false,
            }),
            sink: Box::new(sink),
            stderr: stderr_tail.clone(),
            stderr_reader: Mutex::new(tail::collect(stderr, stderr_tail).ok()),
            ended: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        supervisor.on_stop(spawned.id, move || {
            if let Some(shared) = weak.upgrade() {
                shared.stopping.store(true, Ordering::SeqCst);
                shared.close();
            }
        });
        let (url_tx, url_rx) = channel();
        let reader = shared.clone();
        let started = std::thread::Builder::new()
            .name("opencode-stdout".into())
            .spawn(move || reader.read_stdout(stdout, url_tx))
            .and_then(|_| {
                let opener = shared.clone();
                std::thread::Builder::new()
                    .name("opencode-open".into())
                    .spawn(move || open::open(&opener, &url_rx))
            });
        if let Err(e) = started {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start(e.to_string()));
        }
        Ok(Session { shared })
    }

    /// The thread as it is now.
    pub fn snapshot(&self) -> Thread {
        Thread::clone(&self.shared.state().thread)
    }

    /// The thread as it is now, shared with the session, which copies it before its next change.
    pub fn shared_snapshot(&self) -> Arc<Thread> {
        Arc::clone(&self.shared.state().thread)
    }

    /// Reads the thread without copying it.
    pub fn with_thread<R>(&self, read: impl FnOnce(&Thread) -> R) -> R {
        read(&self.shared.state().thread)
    }

    /// What the server and the session offer.
    pub fn capabilities(&self) -> Capabilities {
        self.shared.state().caps.clone()
    }

    /// OpenCode's id of the session, once it is open; resuming names it.
    pub fn session_id(&self) -> Option<String> {
        self.shared.state().session_id.clone()
    }

    /// The supervisor's number for the process.
    pub fn process(&self) -> ProcessId {
        self.shared.process
    }

    /// Whether the session is open and takes messages.
    pub fn is_ready(&self) -> bool {
        !self.is_ended() && self.shared.state().session_id.is_some()
    }

    /// Whether the process has ended.
    pub fn is_ended(&self) -> bool {
        self.shared.is_ended()
    }

    /// Sends `message`: it starts a turn, or while one runs, is added to it (`delivery: steer`).
    pub fn send(&self, message: UserMessage) -> Result<(), SessionError> {
        if self.is_ended() {
            return Err(SessionError::Closed);
        }
        let path = self.shared.session_path("/prompt")?;
        let (running, starting, images) = {
            let state = self.shared.state();
            (state.mapper.is_running(), state.starting, state.caps.images())
        };
        if starting && !running {
            return Err(SessionError::Busy);
        }
        // Attached files are read here, so the state stays unlocked for the event stream meanwhile.
        let body = crate::prompt::body(&message, images, &self.shared.launch.cwd, running);
        let updates = {
            let mut state = self.shared.state();
            let mut updates = Vec::new();
            if running {
                updates.extend(state.mapper.steered(&message, now()));
            } else {
                if state.thread.title().is_empty() {
                    let title = term_thread::title_from(&message.text);
                    if !title.is_empty() {
                        updates.push(Update::Title(title));
                    }
                }
                updates.extend(state.mapper.begin_turn(&message, now()));
                state.starting = true;
            }
            updates
        };
        let user = crate::mapping::user_key(&updates);
        self.shared.apply(updates);
        let after: After = Box::new(move |result, shared| {
            let updates = {
                let mut state = shared.state();
                if !running {
                    state.starting = false;
                }
                match result {
                    Ok(answer) => {
                        let id = answer.pointer("/data/id").and_then(Value::as_str);
                        user.zip(id)
                            .map(|(key, id)| crate::mapping::chain(key, id))
                            .into_iter()
                            .collect()
                    }
                    Err(error) => {
                        let mut updates = state.mapper.error(ErrorKind::Api, error.to_string(), now());
                        if !running && !state.mapper.is_running() {
                            state.interrupt = false;
                            updates.extend(state.mapper.abandon(now()));
                        }
                        updates
                    }
                }
            };
            shared.apply(updates);
            let interrupt = {
                let mut state = shared.state();
                state.mapper.is_running() && std::mem::take(&mut state.interrupt)
            };
            if interrupt {
                shared.interrupt();
            }
        });
        drop(self.shared.call("POST", path, Some(body), Some(after), unit));
        Ok(())
    }

    /// Asks OpenCode to stop the turn; open requests are refused and withdrawn.
    pub fn cancel(&self) {
        if self.is_ended() {
            return;
        }
        self.shared.withdraw_asks();
        let running = {
            let mut state = self.shared.state();
            let running = state.mapper.is_running();
            if !running && state.starting {
                state.interrupt = true;
            }
            running
        };
        if running {
            self.shared.interrupt();
        }
    }

    /// Ends the session: the server's input closes, which ends it, then the supervisor stops the process after
    /// `grace`.
    pub fn stop(&self, grace: Duration) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.close();
        self.shared.supervisor.stop(self.shared.process, grace);
    }
}
