//! A running `codex app-server`: the process the supervisor started, the lines on its pipes, the requests either
//! side waits on, and the thread the notifications fill.
//!
//! Reading and writing the pipes run on threads of their own; the methods only queue a line or take a short lock,
//! so the UI never waits for Codex.

mod answer;
mod manage;
mod open;
mod reply;
mod tasks;
mod transport;

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use term_process::framing::{self, Line};
use term_process::tail::{self, Tail};
use term_process::{Kind, ProcessId, Supervisor};
use term_thread::{Change, ErrorKind, RequestState, Status, Thread, Update, UserMessage};

use crate::capabilities::Capabilities;
use crate::launch::{ApprovalPolicy, Launch, SandboxMode};
use crate::mapping::Mapper;
use crate::protocol::{method, text};
use crate::requests::Offer;
use crate::version::Version;
use crate::wire::{Id, Message, RpcError};
use transport::{After, Pending, WRITE_QUEUE};

pub(crate) use open::initialize_params;
pub use reply::Reply;
pub(crate) use reply::typed;

/// What the stderr tail keeps.
const STDERR_LIMIT: usize = 64 * 1024;

/// How much of the server's output one read takes in, as much as a pipe holds.
const READ_BUFFER: usize = 64 * 1024;

/// How long the end of a session waits for the stderr of a server that exited on its own.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// The longest message read from the server.
const MAX_MESSAGE: usize = 256 * 1024 * 1024;

/// What a session tells the UI, on one of the session's threads.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The thread changed; read it with `Session::with_thread`.
    Changed(Vec<Change>),
    /// What the server, the account or the thread offers changed.
    Capabilities,
    /// Something keeps the session from working, such as a missing sign-in or a release too old.
    Failed(SessionError),
    /// A sign-in that `Session::login` started has ended.
    Login {
        /// The id `login` returned.
        login_id: Option<String>,
        /// Whether it succeeded.
        success: bool,
        /// Why not.
        error: Option<String>,
    },
    /// The OAuth sign-in of an MCP server has ended.
    McpLogin {
        /// The server.
        server: String,
        /// Whether it succeeded.
        success: bool,
        /// Why not.
        error: Option<String>,
    },
    /// The process ended; its last error output is attached.
    Ended {
        /// The end of what the server wrote to stderr.
        stderr: String,
    },
}

/// Why a session cannot do what was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionError {
    /// The program is not there.
    NotFound(PathBuf),
    /// The process did not start.
    Start(String),
    /// The installed Codex is older than this build was tested against, or its version could not be read; the
    /// UI falls back to `codex-acp`.
    Unsupported {
        /// What `codex --version` named.
        found: Option<Version>,
        /// The oldest release tested.
        tested: Version,
    },
    /// Codex needs an OpenAI sign-in before a turn can run; `Session::login` starts one.
    SignIn,
    /// The server answered a request with an error.
    Server(RpcError),
    /// The server's answer has another shape than the protocol describes.
    Malformed(String),
    /// No request with this id waits for this kind of answer.
    UnknownRequest(String),
    /// The session has no setting or mode with this id.
    UnknownOption(String),
    /// A turn is being started; a message can follow once it runs.
    Busy,
    /// Too many requests or writes are waiting for the server.
    Congested,
    /// No answer came in time.
    Timeout,
    /// The session has ended or has not opened its thread yet.
    Closed,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(path) => write!(f, "{} is not there", path.display()),
            SessionError::Start(why) => write!(f, "codex did not start: {why}"),
            SessionError::Unsupported {
                found: Some(found),
                tested,
            } => {
                write!(f, "codex {found} is older than {tested}")
            }
            SessionError::Unsupported { found: None, .. } => f.write_str("the version of codex is unknown"),
            SessionError::SignIn => f.write_str("codex asks for a sign-in"),
            SessionError::Server(error) => write!(f, "{error}"),
            SessionError::Malformed(why) => write!(f, "codex answered unexpectedly: {why}"),
            SessionError::UnknownRequest(id) => write!(f, "no request {id} waits for this answer"),
            SessionError::UnknownOption(id) => write!(f, "codex has no setting {id}"),
            SessionError::Busy => f.write_str("a turn is starting"),
            SessionError::Congested => f.write_str("the Codex request queue is full"),
            SessionError::Timeout => f.write_str("codex did not answer in time"),
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
    /// No option: a command or change is refused and the turn stops (`cancel`), an MCP server hears `cancel`.
    Dismiss,
}

type Sink = Box<dyn Fn(Event) + Send + Sync>;
struct Ask {
    id: Id,
    offer: Offer,
    thread: Option<String>,
}

/// Settings changed since the last turn, which the next `turn/start` carries and Codex keeps from then on.
#[derive(Debug, Clone, Default, PartialEq)]
struct Overrides {
    model: Option<String>,
    effort: Option<Option<String>>,
    approval: Option<ApprovalPolicy>,
    sandbox: Option<SandboxMode>,
}

impl Overrides {
    fn write(&self, params: &mut Value) {
        if let Some(model) = &self.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &self.effort {
            params["effort"] = json!(effort);
        }
        if let Some(approval) = self.approval {
            params["approvalPolicy"] = json!(approval.wire());
        }
        if let Some(sandbox) = self.sandbox {
            params["sandboxPolicy"] = sandbox.policy();
        }
    }

    /// Puts back what a failed turn did not apply, unless it was changed again meanwhile.
    fn restore(&mut self, old: Overrides) {
        self.model = self.model.take().or(old.model);
        self.effort = self.effort.take().or(old.effort);
        self.approval = self.approval.or(old.approval);
        self.sandbox = self.sandbox.or(old.sandbox);
    }
}

struct State {
    mapper: Mapper,
    thread: Arc<Thread>,
    caps: Capabilities,
    /// Codex's id for the thread, once it opened it.
    thread_id: Option<String>,
    asks: HashMap<String, Ask>,
    overrides: Overrides,
    /// `turn/start` went out and the turn has no id yet.
    starting: bool,
    /// The user stopped a turn that had no id yet.
    interrupt: bool,
    early_requests: Vec<tasks::EarlyRequest>,
}

struct Shared {
    supervisor: Supervisor,
    process: ProcessId,
    launch: Launch,
    /// What goes to the server's input, already framed.
    writer: Mutex<Option<SyncSender<Vec<u8>>>>,
    state: Mutex<State>,
    pending: Mutex<HashMap<i64, Pending>>,
    sink: Sink,
    next_id: AtomicI64,
    next_ask: AtomicU64,
    stderr: Arc<Mutex<Tail>>,
    stderr_reader: Mutex<Option<tail::Collecting>>,
    ended: AtomicBool,
    stopping: AtomicBool,
}

/// A running Codex session; clones share it. It runs until `stop`, the supervisor ends it or the server exits.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn pending(&self) -> MutexGuard<'_, HashMap<i64, Pending>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Reads the server's output until it ends.
    fn read(self: Arc<Self>, stdout: ChildStdout) {
        let mut reader = BufReader::with_capacity(READ_BUFFER, stdout);
        let mut line = Vec::new();
        loop {
            match framing::read_line(&mut reader, &mut line, MAX_MESSAGE) {
                Ok(Line::End) | Err(_) => break,
                Ok(Line::TooLong) => {}
                Ok(Line::Read) => {
                    let Ok(text) = std::str::from_utf8(&line) else {
                        break;
                    };
                    if let Some(message) = Message::parse(text) {
                        self.dispatch(message);
                    }
                }
            }
        }
        self.finish();
    }

    fn dispatch(self: &Arc<Self>, message: Message) {
        match message {
            Message::Response {
                id: Id::Number(id),
                result,
            } => self.answered(id, Ok(result)),
            Message::Error {
                id: Some(Id::Number(id)),
                error,
            } => self.answered(id, Err(SessionError::Server(error))),
            Message::Notification { method, params } => self.on_notification(&method, &params),
            Message::Request { id, method, params } => self.on_request(id, &method, &params),
            Message::Response { .. } | Message::Error { .. } => {}
        }
    }

    fn on_notification(self: &Arc<Self>, method: &str, params: &Value) {
        match method {
            "account/login/completed" => {
                self.emit(Event::Login {
                    login_id: text(params, "loginId").map(str::to_string),
                    success: params.get("success").and_then(Value::as_bool).unwrap_or(false),
                    error: text(params, "error").map(str::to_string),
                });
                return;
            }
            "mcpServer/oauthLogin/completed" => {
                self.emit(Event::McpLogin {
                    server: text(params, "name").unwrap_or_default().to_string(),
                    success: params.get("success").and_then(Value::as_bool).unwrap_or(false),
                    error: text(params, "error").map(str::to_string),
                });
                return;
            }
            "serverRequest/resolved" => {
                if let Some(id) = params
                    .get("requestId")
                    .and_then(|i| serde_json::from_value::<Id>(i.clone()).ok())
                {
                    self.resolved(&id);
                }
                return;
            }
            "turn/started" if text(params, "threadId") == self.state().thread_id.as_deref() => {
                self.state().starting = false;
            }
            _ => {}
        }
        let (mut updates, learned) = {
            let mut state = self.state();
            let State { mapper, caps, .. } = &mut *state;
            mapper.notification(method, params, caps, now())
        };
        if method == "turn/completed" && !self.state().asks.is_empty() {
            updates.push(Update::Status(Status::RequiresAction));
        }
        self.apply(updates);
        if learned {
            self.emit(Event::Capabilities);
        }
        self.replay_task_requests();
        if method == "turn/started"
            && text(params, "threadId") == self.state().thread_id.as_deref()
            && std::mem::take(&mut self.state().interrupt)
        {
            self.interrupt();
        }
    }

    fn on_request(self: &Arc<Self>, id: Id, method: &str, params: &Value) {
        if self.defer_task_request(&id, method, params) {
            return;
        }
        let ask = format!("p{}", self.next_ask.fetch_add(1, Ordering::SeqCst) + 1);
        let taken = self.state().mapper.task_request(&ask, method, params, now());
        let Some((offer, updates)) = taken else {
            let error = RpcError::new(RpcError::METHOD_NOT_FOUND, format!("{method} is not supported"));
            let _ = self.write(&Message::Error { id: Some(id), error });
            return;
        };
        self.state().asks.insert(
            ask,
            Ask {
                id,
                offer,
                thread: text(params, "threadId").map(str::to_string),
            },
        );
        self.apply(updates);
    }

    /// The server no longer needs the answer to its request `id`, such as when the turn was stopped.
    fn resolved(&self, id: &Id) {
        let updates = {
            let mut state = self.state();
            state.early_requests.retain(|request| &request.id != id);
            let Some(ask) = state.asks.iter().find(|(_, a)| a.id == *id).map(|(k, _)| k.clone()) else {
                return;
            };
            let Some(ask) = state.asks.remove(&ask) else {
                return;
            };
            let mut updates = vec![Update::Resolve {
                key: ask.offer.key,
                state: RequestState::Withdrawn,
            }];
            if state.asks.is_empty() && state.thread.status() == Status::RequiresAction {
                updates.push(Update::Status(if state.mapper.turn().is_some() || state.starting {
                    Status::Running
                } else {
                    Status::Idle
                }));
            }
            updates
        };
        self.apply(updates);
    }

    /// Dismisses the main turn's requests; a child's pending approval belongs to that child, not to Stop Turn.
    fn withdraw_asks(&self) {
        let asks = {
            let mut state = self.state();
            let (ours, children): (HashMap<_, _>, HashMap<_, _>) = std::mem::take(&mut state.asks)
                .into_iter()
                .partition(|(_, ask)| ask.thread.is_none() || ask.thread == state.thread_id);
            state.asks = children;
            ours.into_values().collect::<Vec<_>>()
        };
        let mut updates = Vec::new();
        for ask in asks {
            let _ = self.respond(ask.id, crate::requests::response(&ask.offer.kind, None));
            updates.push(Update::Resolve {
                key: ask.offer.key,
                state: RequestState::Withdrawn,
            });
        }
        self.apply(updates);
    }

    fn interrupt(self: &Arc<Self>) {
        let (thread, turn) = {
            let state = self.state();
            (state.thread_id.clone(), state.mapper.turn().map(str::to_string))
        };
        if let (Some(thread), Some(turn)) = (thread, turn) {
            let params = json!({"threadId": thread, "turnId": turn});
            drop(self.call(method::TURN_INTERRUPT, params, None, reply::unit));
        }
    }

    /// Gives the stderr reader a moment to take in what the server wrote just before it exited.
    fn await_stderr(&self) {
        if let Some(reader) = self.stderr_reader.lock().unwrap_or_else(|e| e.into_inner()).take() {
            reader.wait(LAST_WORDS);
        }
    }

    /// The server's output ended: the process is gone or was stopped.
    fn finish(&self) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.writer.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.pending().clear();
        {
            let mut state = self.state();
            state.starting = false;
            state.interrupt = false;
        }
        let withdrawn: Vec<Update> = self
            .state()
            .asks
            .drain()
            .map(|(_, ask)| Update::Resolve {
                key: ask.offer.key,
                state: RequestState::Withdrawn,
            })
            .collect();
        self.apply(withdrawn);
        let stopping = self.stopping.load(Ordering::SeqCst);
        if !stopping {
            self.await_stderr();
        }
        let stderr = self.stderr.lock().unwrap_or_else(|e| e.into_inner()).text();
        let running = self.state().thread.status() != Status::Idle;
        if running {
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
        let updates = self.state().mapper.end_tasks(now());
        self.apply(updates);
        self.supervisor.stop(self.process, Duration::from_millis(500));
        self.emit(Event::Ended { stderr });
    }
}

/// Writes what `rx` queues to the server's input.
fn write_lines(mut stdin: ChildStdin, rx: &Receiver<Vec<u8>>) {
    while let Ok(bytes) = rx.recv() {
        let written = stdin.write_all(&bytes).and_then(|()| stdin.flush());
        if written.is_err() {
            break;
        }
    }
}

impl Session {
    /// Starts `codex app-server` as `launch` describes, filling `thread`; the version check, the handshake and
    /// opening the thread follow on a thread of their own.
    ///
    /// ```no_run
    /// use std::sync::mpsc::channel;
    ///
    /// use term_codex::{Event, Launch, Session};
    /// use term_process::Supervisor;
    /// use term_thread::{Thread, ThreadId};
    ///
    /// let env: Vec<(String, String)> = std::env::vars().collect();
    /// let launch = Launch::new("/opt/homebrew/bin/codex", env, "/work");
    /// let thread = Thread::new(ThreadId::from("t1"), term_codex::AGENT, "/work");
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
        let spawned = supervisor
            .spawn(launch.command(), Kind::Agent, launch.name.clone())
            .map_err(|e| SessionError::Start(e.to_string()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (spawned.stdin, spawned.stdout, spawned.stderr) else {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start("the pipes are missing".into()));
        };
        let origin = u64::try_from(now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos()).unwrap_or_default();
        let (tx, rx) = sync_channel(WRITE_QUEUE);
        let stderr_tail = Arc::new(Mutex::new(Tail::new(STDERR_LIMIT)));
        let shared = Arc::new(Shared {
            supervisor: supervisor.clone(),
            process: spawned.id,
            launch,
            writer: Mutex::new(Some(tx)),
            state: Mutex::new(State {
                mapper: Mapper::new(origin),
                thread: Arc::new(thread),
                caps: Capabilities::default(),
                thread_id: None,
                asks: HashMap::new(),
                overrides: Overrides::default(),
                starting: false,
                interrupt: false,
                early_requests: Vec::new(),
            }),
            pending: Mutex::new(HashMap::new()),
            sink: Box::new(sink),
            next_id: AtomicI64::new(1),
            next_ask: AtomicU64::new(0),
            stderr: stderr_tail.clone(),
            stderr_reader: Mutex::new(tail::collect(stderr, stderr_tail).ok()),
            ended: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
        });
        let weak = Arc::downgrade(&shared);
        supervisor.on_stop(spawned.id, move || {
            if let Some(shared) = weak.upgrade() {
                shared.stopping.store(true, Ordering::SeqCst);
                *shared.writer.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        });
        let started = std::thread::Builder::new()
            .name("codex-writer".into())
            .spawn(move || write_lines(stdin, &rx))
            .and_then(|_| {
                let reader = shared.clone();
                std::thread::Builder::new()
                    .name("codex-reader".into())
                    .spawn(move || reader.read(stdout))
            })
            .and_then(|_| {
                let opener = shared.clone();
                std::thread::Builder::new()
                    .name("codex-open".into())
                    .spawn(move || open::open(&opener))
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

    /// What the server, the account and the thread offer.
    pub fn capabilities(&self) -> Capabilities {
        self.shared.state().caps.clone()
    }

    /// Codex's id for the thread, once it opened it; resuming names it.
    pub fn session_id(&self) -> Option<String> {
        self.shared.state().thread_id.clone()
    }

    /// How the session was started, with the program and environment Codex's other commands take too.
    pub fn launch(&self) -> &Launch {
        &self.shared.launch
    }

    /// The supervisor's number for the process.
    pub fn process(&self) -> ProcessId {
        self.shared.process
    }

    /// Whether the thread is open and takes messages.
    pub fn is_ready(&self) -> bool {
        !self.is_ended() && self.shared.state().thread_id.is_some()
    }

    /// Whether the process has ended.
    pub fn is_ended(&self) -> bool {
        self.shared.ended.load(Ordering::SeqCst)
    }

    /// The latest unified diff of what the running or last turn changed.
    pub fn turn_diff(&self) -> Option<String> {
        self.shared.state().mapper.diff().map(str::to_string)
    }

    fn thread_id(&self) -> Result<String, SessionError> {
        if self.is_ended() {
            return Err(SessionError::Closed);
        }
        self.shared.state().thread_id.clone().ok_or(SessionError::Closed)
    }

    /// Sends `message`: it starts a turn, or while one runs, is added to it (`turn/steer`).
    pub fn send(&self, message: UserMessage) -> Result<(), SessionError> {
        self.send_turn(message, None)
    }

    /// Starts a turn whose final answer follows the JSON schema `schema` (`outputSchema` of `turn/start`, C63);
    /// while a turn runs it is `Busy`, as steering takes no schema.
    pub fn send_with_schema(&self, message: UserMessage, schema: Value) -> Result<(), SessionError> {
        self.send_turn(message, Some(schema))
    }

    fn send_turn(&self, message: UserMessage, schema: Option<Value>) -> Result<(), SessionError> {
        let thread = self.thread_id()?;
        let (running, starting) = {
            let state = self.shared.state();
            (state.mapper.turn().map(str::to_string), state.starting)
        };
        if let Some(turn) = running {
            if schema.is_some() {
                return Err(SessionError::Busy);
            }
            return self.steer(&thread, &turn, &message);
        }
        if starting {
            return Err(SessionError::Busy);
        }
        let input = self.input(&message);
        let (params, overrides, updates) = {
            let mut state = self.shared.state();
            if state.starting || state.mapper.turn().is_some() {
                return Err(SessionError::Busy);
            }
            let mut updates = Vec::new();
            if state.thread.title().is_empty() {
                let title = term_thread::title_from(&message.text);
                if !title.is_empty() {
                    updates.push(Update::Title(title));
                }
            }
            let (key, begun) = state.mapper.begin_turn(&message, now());
            updates.extend(begun);
            let overrides = std::mem::take(&mut state.overrides);
            let mut params = json!({"threadId": thread});
            params["input"] = Value::Array(input);
            params["clientUserMessageId"] = json!(key);
            overrides.write(&mut params);
            if let Some(schema) = schema {
                params["outputSchema"] = schema;
            }
            state.starting = true;
            (params, overrides, updates)
        };
        self.shared.apply(updates);
        let after: After = Box::new(move |result, shared| {
            let updates = {
                let mut state = shared.state();
                state.starting = false;
                match result {
                    Ok(result) => {
                        if let Some(turn) = result.get("turn").and_then(|t| text(t, "id")) {
                            state.mapper.turn_answered(turn);
                        }
                        Vec::new()
                    }
                    Err(error) => {
                        // A stop asked for while this turn was starting has nothing left to stop.
                        state.interrupt = false;
                        state.overrides.restore(overrides);
                        let (kind, message) = match error {
                            SessionError::Server(error) => (
                                error.data.as_ref().map_or(ErrorKind::Api, crate::mapping::error_kind),
                                error.message.clone(),
                            ),
                            error => (ErrorKind::Process, error.to_string()),
                        };
                        let mut updates = state.mapper.error(kind, message, now());
                        updates.extend(state.mapper.abandon(now()));
                        updates
                    }
                }
            };
            shared.apply(updates);
        });
        self.shared
            .call(method::TURN_START, params, Some(after), reply::unit)
            .submitted()
    }

    /// The `input` items of `message`, built without the state's lock, as they read files and encode images.
    fn input(&self, message: &UserMessage) -> Vec<Value> {
        let images = self.shared.state().caps.images();
        crate::prompt::input(message, images, &self.shared.launch.cwd)
    }

    fn steer(&self, thread: &str, turn: &str, message: &UserMessage) -> Result<(), SessionError> {
        let input = self.input(message);
        let (params, updates) = {
            let mut state = self.shared.state();
            let (key, updates) = state.mapper.steered(message, now());
            let mut params = json!({"threadId": thread});
            params["input"] = Value::Array(input);
            params["expectedTurnId"] = json!(turn);
            params["clientUserMessageId"] = json!(key);
            (params, updates)
        };
        self.shared.apply(updates);
        let after: After = Box::new(|result, shared| {
            if let Err(error) = result {
                let updates = shared.state().mapper.error(ErrorKind::Turn, error.to_string(), now());
                shared.apply(updates);
            }
        });
        self.shared
            .call(method::TURN_STEER, params, Some(after), reply::unit)
            .submitted()
    }

    /// Asks Codex to stop the turn; open requests are answered as dismissed and withdrawn.
    pub fn cancel(&self) {
        if self.is_ended() {
            return;
        }
        self.shared.withdraw_asks();
        let known = {
            let mut state = self.shared.state();
            let known = state.mapper.turn().is_some();
            if !known && state.starting {
                state.interrupt = true;
            }
            known
        };
        if known {
            self.shared.interrupt();
        }
    }

    /// Ends the session: the server's input closes, then the supervisor stops the process after `grace`.
    pub fn stop(&self, grace: Duration) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        *self.shared.writer.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.shared.supervisor.stop(self.shared.process, grace);
    }
}
