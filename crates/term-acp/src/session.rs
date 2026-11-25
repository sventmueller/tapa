//! A running ACP agent: the process the supervisor started, the protocol connection over its pipes, and the thread
//! the connection fills.
//!
//! The connection runs on a thread of its own, and every request waits for its answer as a task of that
//! connection, so neither the UI nor the connection's dispatch ever waits for the agent. Opening a session and the
//! settings have deadlines; a turn runs until the agent ends it or the user cancels it.

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, BufReader, Read};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant, SystemTime};
use term_base::LockIgnoringPoison;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema as acp;
use agent_client_protocol::{Agent, Builder, ByteStreams, Client, ConnectionTo, JsonRpcRequest, Responder};
use futures::channel::oneshot;
use term_process::framing::{self, Line};
use term_process::tail::{self, Tail};
use term_process::{Kind, ProcessId, Supervisor};
use term_thread::{Change, Decision, ErrorKind, RequestState, Status, Thread, Update, UserMessage};

use crate::capabilities::{Capabilities, Learn};
use crate::launch::Launch;
use crate::mapping::Mapper;
use crate::prompt;

mod extras;

/// What the stderr tail keeps.
const STDERR_LIMIT: usize = 64 * 1024;

/// How long the end of a session waits for the stderr of an agent that exited on its own.
const LAST_WORDS: Duration = Duration::from_millis(500);

/// How long the agent's output may stay open after its process is gone; what it wrote last is still read meanwhile.
const EXIT_GRACE: Duration = Duration::from_secs(1);

/// How long the agent may take to answer `initialize`.
const INITIALIZE: Duration = Duration::from_secs(60);

/// How long the agent may take to open a session; loading one replays the whole conversation, so it gets longer.
const NEW_SESSION: Duration = Duration::from_secs(60);
const LOAD_SESSION: Duration = Duration::from_secs(180);

/// How long the agent may take to switch a mode or an option.
const SETTING: Duration = Duration::from_secs(30);

/// The longest line of the agent's output that is read; a longer one is skipped, so that it costs neither the memory
/// nor the session.
const LINE_LIMIT: usize = 64 * 1024 * 1024;

/// The agent's output as the connection reads it: whole lines, without those longer than `limit`.
struct Lines<R> {
    reader: BufReader<R>,
    limit: usize,
    line: Vec<u8>,
    at: usize,
}

impl<R: Read> Lines<R> {
    fn new(inner: R, limit: usize) -> Lines<R> {
        Lines {
            reader: BufReader::with_capacity(64 * 1024, inner),
            limit,
            line: Vec::new(),
            at: 0,
        }
    }
}

impl<R: Read> Read for Lines<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.at == self.line.len() {
            if self.line.capacity() > 1024 * 1024 {
                self.line = Vec::new();
            }
            self.at = 0;
            match framing::read_line(&mut self.reader, &mut self.line, self.limit)? {
                Line::Read => {}
                Line::TooLong => term_base::diag::report(
                    "acp: a line of the agent was skipped",
                    format_args!("it is longer than {} MiB", self.limit >> 20),
                ),
                Line::End => return Ok(0),
            }
        }
        let n = buf.len().min(self.line.len() - self.at);
        buf[..n].copy_from_slice(&self.line[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// What a session tells the UI, on one of the session's threads.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The thread changed; read it with `Session::snapshot`.
    Changed(Vec<Change>),
    /// What the agent or its session offers changed.
    Capabilities,
    /// Something keeps the session from working.
    Failed(SessionError),
    /// The process ended; its last error output is attached.
    Ended {
        /// The end of what the agent wrote to stderr.
        stderr: String,
    },
}

/// Why a session cannot do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The program is not there.
    NotFound(PathBuf),
    /// The process did not start.
    Start(String),
    /// The agent, by name, cannot skip the project's configuration and the workspace is not trusted (E16).
    Untrusted(String),
    /// The agent wants a sign-in before it opens a session; the methods are in the capabilities.
    SignIn,
    /// The agent speaks a protocol version this build does not.
    Version(u16),
    /// The agent answered a request with an error.
    Agent(String),
    /// The agent did not answer this request in time.
    Timeout(&'static str),
    /// No permission request with this id waits for an answer.
    UnknownRequest(String),
    /// The session has no live connection.
    Closed,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(path) => write!(f, "{} is not there", path.display()),
            SessionError::Start(why) => write!(f, "the agent did not start: {why}"),
            SessionError::Untrusted(name) => write!(f, "{name} would load the project's configuration"),
            SessionError::SignIn => write!(f, "the agent asks for a sign-in"),
            SessionError::Version(v) => write!(f, "the agent speaks protocol version {v}"),
            SessionError::Agent(why) => write!(f, "{why}"),
            SessionError::Timeout(method) => write!(f, "the agent did not answer {method} in time"),
            SessionError::UnknownRequest(id) => write!(f, "no request {id} waits for an answer"),
            SessionError::Closed => write!(f, "the session has ended"),
        }
    }
}

impl term_base::explain::Explain for SessionError {
    fn explain(&self) -> String {
        use splat_i18n::{t, tf};
        match self {
            SessionError::NotFound(path) => tf("{} wurde nicht gefunden.", &[&path.display().to_string()]),
            SessionError::Start(why) => tf("Der Agent ist nicht gestartet: {}", &[why]),
            SessionError::Untrusted(name) => tf("{} startet nur in einem vertrauten Arbeitsbereich.", &[name]),
            SessionError::SignIn => t("Der Agent verlangt eine Anmeldung.").to_string(),
            SessionError::Version(version) => tf(
                "Der Agent spricht die Protokollversion {}, diese App Version 1",
                &[&version.to_string()],
            ),
            SessionError::Agent(why) => why.clone(),
            SessionError::Timeout(method) => tf("Der Agent hat auf {} nicht rechtzeitig geantwortet.", &[method]),
            SessionError::UnknownRequest(_) => t("Diese Anfrage wartet nicht mehr auf eine Antwort.").to_string(),
            SessionError::Closed => t("Die Sitzung ist beendet.").to_string(),
        }
    }
}

impl std::error::Error for SessionError {}

/// The user's answer to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// The option at this place in the request's list.
    Option(usize),
    /// No option; the agent hears that the request was cancelled.
    Dismiss,
}

struct Ask {
    responder: Responder<acp::RequestPermissionResponse>,
    options: Vec<acp::PermissionOption>,
    /// Whether it is about a git command that destroys work, for which no rule may be stored (E27).
    destructive: bool,
}

/// The option `choice` stands for; for a destructive git command, allowing always counts as allowing once, so that
/// the agent stores no rule for it.
fn picked(options: &[acp::PermissionOption], choice: Choice, destructive: bool) -> Option<&acp::PermissionOption> {
    let Choice::Option(n) = choice else {
        return None;
    };
    match options.get(n) {
        Some(option) if destructive && option.kind == acp::PermissionOptionKind::AllowAlways => {
            options.iter().find(|o| o.kind == acp::PermissionOptionKind::AllowOnce)
        }
        other => other,
    }
}

struct State {
    mapper: Mapper,
    thread: Arc<Thread>,
    caps: Capabilities,
    session: Option<acp::SessionId>,
    asks: HashMap<String, Ask>,
    /// The fork answering a question beside the conversation, while it does.
    side: Option<extras::Side>,
}

type Sink = Box<dyn Fn(Event) + Send + Sync>;

struct Shared {
    supervisor: Supervisor,
    process: ProcessId,
    launch: Launch,
    state: Mutex<State>,
    sink: Sink,
    connection: Mutex<Option<ConnectionTo<Agent>>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    stderr: Arc<Mutex<Tail>>,
    stderr_reader: Mutex<Option<tail::Collecting>>,
    ended: AtomicBool,
    stopping: AtomicBool,
    next_ask: AtomicU64,
}

/// A running ACP session; clones share it. It runs until `stop`, the supervisor ends it or the agent exits.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock_ignoring_poison()
    }

    /// Tells the UI `event`; a panic in the sink costs that event, not the connection.
    fn emit(&self, event: Event) {
        term_base::lane::contained(|| (self.sink)(event));
    }

    fn connection(&self) -> Option<ConnectionTo<Agent>> {
        self.connection.lock_ignoring_poison().clone()
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

    fn on_update(&self, notification: acp::SessionNotification) {
        let (updates, learned) = {
            let mut state = self.state();
            if let Some(side) = state.side.as_mut().filter(|side| side.is(&notification.session_id)) {
                side.take(&notification.update);
                return;
            }
            let State { mapper, caps, .. } = &mut *state;
            mapper.update(&notification.update, caps, now())
        };
        self.apply(updates);
        if learned {
            self.emit(Event::Capabilities);
        }
    }

    fn on_permission(
        &self,
        request: acp::RequestPermissionRequest,
        responder: Responder<acp::RequestPermissionResponse>,
    ) {
        let aside = self
            .state()
            .side
            .as_ref()
            .is_some_and(|side| side.is(&request.session_id));
        if request.options.is_empty() || aside {
            // Nothing to choose from means nothing the user could allow, and a question beside the conversation runs
            // no tools; either request is refused (E21, E146).
            let _ = responder.respond(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            ));
            return;
        }
        let id = format!("p{}", self.next_ask.fetch_add(1, Ordering::SeqCst) + 1);
        let updates = {
            let mut state = self.state();
            let updates = state
                .mapper
                .permission(&id, &request.tool_call, &request.options, now());
            let destructive = state.mapper.is_destructive(&request.tool_call);
            state.asks.insert(
                id,
                Ask {
                    responder,
                    options: request.options,
                    destructive,
                },
            );
            updates
        };
        self.apply(updates);
    }

    /// Answers every open permission request as cancelled, as the protocol asks when a turn is cancelled.
    fn withdraw_asks(&self) {
        let asks: Vec<(String, Ask)> = self.state().asks.drain().collect();
        let mut updates = Vec::new();
        for (id, ask) in asks {
            let _ = ask.responder.respond(acp::RequestPermissionResponse::new(
                acp::RequestPermissionOutcome::Cancelled,
            ));
            updates.push(Update::Resolve {
                key: format!("permission-{id}"),
                state: RequestState::Withdrawn,
            });
        }
        self.apply(updates);
    }

    fn fail(&self, error: SessionError) {
        self.emit(Event::Failed(error));
    }

    /// Gives the stderr reader a moment to take in what the agent wrote just before it exited.
    fn await_stderr(&self) {
        if let Some(reader) = self.stderr_reader.lock_ignoring_poison().take() {
            reader.wait(LAST_WORDS);
        }
    }

    /// The connection ended: the process is gone or was stopped.
    fn finish(&self, result: Result<(), agent_client_protocol::Error>) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.connection.lock_ignoring_poison() = None;
        self.withdraw_asks();
        let stopping = self.stopping.load(Ordering::SeqCst);
        if !stopping {
            self.await_stderr();
        }
        let stderr = self.stderr.lock_ignoring_poison().text();
        let running = self.state().thread.status() != Status::Idle;
        if stopping && running {
            let mut updates = self.state().mapper.settle(now());
            updates.push(Update::Status(Status::Idle));
            self.apply(updates);
        }
        if !stopping && (running || result.is_err()) {
            let message = match &result {
                Err(e) if stderr.trim().is_empty() => e.message.clone(),
                _ => stderr
                    .trim()
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            let mut updates = self.state().mapper.error(ErrorKind::Process, message, now());
            updates.push(Update::Status(Status::Idle));
            self.apply(updates);
        }
        self.supervisor.stop(self.process, Duration::from_millis(500));
        self.emit(Event::Ended { stderr });
    }
}

/// Whether `error` only says that the connection is gone, which `finish` reports with the agent's last words.
fn closed(error: &agent_client_protocol::Error) -> bool {
    agent_client_protocol::is_incoming_transport_closed(error) || agent_client_protocol::is_connection_terminated(error)
}

/// `future`'s output, or `None` when `limit` passes first.
async fn within<T>(limit: Duration, future: impl Future<Output = T>) -> Option<T> {
    let (fire, fired) = oneshot::channel::<()>();
    term_base::timer::after(limit, move || {
        let _ = fire.send(());
    });
    match futures::future::select(std::pin::pin!(future), fired).await {
        futures::future::Either::Left((output, _)) => Some(output),
        futures::future::Either::Right(_) => None,
    }
}

/// The error a request that passed its deadline ends with.
fn timed_out(method: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::new(
        acp::ErrorCode::InternalError,
        format!("the agent did not answer {method} in time"),
    )
}

/// Sends `request` and waits for the answer, up to `limit` when given, as a task of the connection; hands it to
/// `then` there. When the connection takes no more tasks, `then` hears that at once.
fn ask<Req>(
    connection: &ConnectionTo<Agent>,
    request: Req,
    limit: Option<Duration>,
    then: impl FnOnce(Result<Req::Response, agent_client_protocol::Error>) + Send + 'static,
) where
    Req: JsonRpcRequest + Send + 'static,
    Req::Response: Send + 'static,
{
    let method = request.method().to_string();
    let sent = connection.send_request(request);
    let slot = Arc::new(Mutex::new(Some(then)));
    let theirs = slot.clone();
    let task = async move {
        let answer = match limit {
            Some(limit) => within(limit, sent.block_task())
                .await
                .unwrap_or_else(|| Err(timed_out(&method))),
            None => sent.block_task().await,
        };
        let then = theirs.lock_ignoring_poison().take();
        if let Some(then) = then {
            term_base::lane::contained(|| then(answer));
        }
        Ok(())
    };
    if let Err(error) = connection.spawn(task) {
        let then = slot.lock_ignoring_poison().take();
        if let Some(then) = then {
            term_base::lane::contained(|| then(Err(error)));
        }
    }
}

impl Session {
    /// Starts the agent as `launch` describes, filling `thread`; the connection opens a session once the agent
    /// answered `initialize`.
    ///
    /// ```no_run
    /// use std::sync::mpsc::channel;
    ///
    /// use term_acp::{Event, Launch, Session};
    /// use term_process::Supervisor;
    /// use term_thread::{Thread, ThreadId};
    ///
    /// let env: Vec<(String, String)> = std::env::vars().collect();
    /// let launch = Launch::new("gemini", "Gemini CLI", "/opt/homebrew/bin/gemini", vec!["--acp".into()], env, "/work");
    /// let thread = Thread::new(ThreadId::from("t1"), "gemini", "/work");
    /// let (tx, rx) = channel();
    /// let session = Session::start(Supervisor::global(), launch, thread, move |event| drop(tx.send(event)))?;
    /// while !matches!(rx.recv()?, Event::Capabilities) {}
    /// assert_eq!(session.snapshot().agent(), "gemini");
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
        if !launch.may_start() {
            return Err(SessionError::Untrusted(launch.name.clone()));
        }
        let spawned = supervisor
            .spawn(launch.command(), Kind::Agent, launch.name.clone())
            .map_err(|e| SessionError::Start(e.to_string()))?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (spawned.stdin, spawned.stdout, spawned.stderr) else {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start("the pipes are missing".into()));
        };
        let stderr_tail = Arc::new(Mutex::new(Tail::new(STDERR_LIMIT)));
        let stderr_reader = tail::collect(stderr, stderr_tail.clone()).ok();
        let (shutdown, closed) = oneshot::channel();
        let shared = Arc::new(Shared {
            supervisor: supervisor.clone(),
            process: spawned.id,
            launch,
            state: Mutex::new(State {
                mapper: Mapper::new(),
                thread: Arc::new(thread),
                caps: Capabilities::default(),
                session: None,
                asks: HashMap::new(),
                side: None,
            }),
            sink: Box::new(sink),
            connection: Mutex::new(None),
            shutdown: Mutex::new(Some(shutdown)),
            stderr: stderr_tail,
            stderr_reader: Mutex::new(stderr_reader),
            ended: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            next_ask: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&shared);
        supervisor.on_stop(spawned.id, move || {
            if let Some(shared) = weak.upgrade() {
                shared.stopping.store(true, Ordering::SeqCst);
                if let Some(shutdown) = shared.shutdown.lock_ignoring_poison().take() {
                    let _ = shutdown.send(());
                }
            }
        });
        let (watcher, watched) = (supervisor.clone(), Arc::downgrade(&shared));
        let runner = shared.clone();
        let started = term_base::thread::detach("acp-exit", move || watch(&watcher, spawned.id, &watched))
            .map_err(|unstarted| unstarted.error)
            .and_then(|()| {
                term_base::thread::detach(&format!("acp-{}", runner.launch.agent), move || {
                    term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
                    connect(runner, stdin, stdout, closed);
                })
                .map_err(|unstarted| unstarted.error)
            });
        if let Err(error) = started {
            shared.stopping.store(true, Ordering::SeqCst);
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start(error.to_string()));
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

    /// What the agent and its session offer.
    pub fn capabilities(&self) -> Capabilities {
        self.shared.state().caps.clone()
    }

    /// The agent's id for the session, once it opened one.
    pub fn session_id(&self) -> Option<String> {
        self.shared.state().session.as_ref().map(|s| s.0.to_string())
    }

    /// Whether a session is open and takes prompts.
    pub fn is_ready(&self) -> bool {
        !self.is_ended() && self.shared.state().session.is_some()
    }

    /// Whether the process has ended.
    pub fn is_ended(&self) -> bool {
        self.shared.ended.load(Ordering::SeqCst)
    }

    fn ready(&self) -> Result<(ConnectionTo<Agent>, acp::SessionId), SessionError> {
        let connection = self.shared.connection().ok_or(SessionError::Closed)?;
        let session = self.shared.state().session.clone().ok_or(SessionError::Closed)?;
        Ok((connection, session))
    }

    /// Sends `message` as a prompt; the turn runs until the agent answers it.
    pub fn send(&self, message: UserMessage) -> Result<(), SessionError> {
        let (connection, session) = self.ready()?;
        let (blocks, updates) = {
            let mut state = self.shared.state();
            let blocks = prompt::blocks(&message, &state.caps, &self.shared.launch.cwd);
            (blocks, state.mapper.begin_turn(&message, now()))
        };
        self.shared.apply(updates);
        let shared = self.shared.clone();
        ask(
            &connection,
            acp::PromptRequest::new(session, blocks),
            None,
            move |result| {
                if result.as_ref().is_err_and(closed) {
                    return;
                }
                let updates = {
                    let mut state = shared.state();
                    match &result {
                        Ok(response) => {
                            let mut updates = state.mapper.end_turn(response.stop_reason, now());
                            updates.extend(crate::usage::response(response));
                            updates
                        }
                        Err(error) => {
                            let mut updates = state.mapper.error(ErrorKind::Api, error.message.clone(), now());
                            updates.push(Update::Status(Status::Idle));
                            updates
                        }
                    }
                };
                shared.withdraw_asks();
                shared.apply(updates);
            },
        );
        Ok(())
    }

    /// Asks the agent to stop the turn; open permission requests are withdrawn.
    pub fn cancel(&self) {
        let Ok((connection, session)) = self.ready() else {
            return;
        };
        self.shared.withdraw_asks();
        let _ = connection.send_notification(acp::CancelNotification::new(session));
    }

    /// Answers the permission request `request` with `choice`.
    pub fn answer(&self, request: &str, choice: Choice) -> Result<(), SessionError> {
        let ask = self
            .shared
            .state()
            .asks
            .remove(request)
            .ok_or_else(|| SessionError::UnknownRequest(request.to_string()))?;
        let picked = picked(&ask.options, choice, ask.destructive);
        let (outcome, decision) = match picked {
            Some(option) => {
                let decision = match option.kind {
                    acp::PermissionOptionKind::AllowOnce => Decision::Allowed {
                        always: false,
                        edited: false,
                    },
                    acp::PermissionOptionKind::AllowAlways => Decision::Allowed {
                        always: true,
                        edited: false,
                    },
                    _ => Decision::Denied { message: None },
                };
                let selected = acp::SelectedPermissionOutcome::new(option.option_id.clone());
                (acp::RequestPermissionOutcome::Selected(selected), decision)
            }
            None => (
                acp::RequestPermissionOutcome::Cancelled,
                Decision::Denied { message: None },
            ),
        };
        ask.responder
            .respond(acp::RequestPermissionResponse::new(outcome))
            .map_err(|e| SessionError::Agent(e.message))?;
        let still_asking = !self.shared.state().asks.is_empty();
        let mut updates = vec![Update::Resolve {
            key: format!("permission-{request}"),
            state: RequestState::Decided(decision),
        }];
        if !still_asking {
            updates.push(Update::Status(Status::Running));
        }
        self.shared.apply(updates);
        Ok(())
    }

    /// Switches the session to mode `id`.
    pub fn set_mode(&self, id: &str) -> Result<(), SessionError> {
        let (connection, session) = self.ready()?;
        let shared = self.shared.clone();
        let mode = id.to_string();
        ask(
            &connection,
            acp::SetSessionModeRequest::new(session, id.to_string()),
            Some(SETTING),
            move |result| match result {
                Ok(_) => {
                    shared.state().caps.mode = Some(mode.clone());
                    shared.apply(vec![Update::Mode(mode)]);
                    shared.emit(Event::Capabilities);
                }
                Err(error) => shared.fail(SessionError::Agent(error.message)),
            },
        );
        Ok(())
    }

    /// Sets the select option `option` to `value`, such as the model.
    pub fn set_option(&self, option: &str, value: &str) -> Result<(), SessionError> {
        self.configure(
            option,
            acp::SessionConfigOptionValue::ValueId {
                value: value.to_string().into(),
            },
        )
    }

    /// Turns the on/off option `option` on or off.
    pub fn set_flag(&self, option: &str, on: bool) -> Result<(), SessionError> {
        self.configure(option, acp::SessionConfigOptionValue::Boolean { value: on })
    }

    fn configure(&self, option: &str, value: acp::SessionConfigOptionValue) -> Result<(), SessionError> {
        let (connection, session) = self.ready()?;
        let shared = self.shared.clone();
        let request = acp::SetSessionConfigOptionRequest::new(session, option.to_string(), value);
        ask(&connection, request, Some(SETTING), move |result| match result {
            Ok(response) => {
                let updates = {
                    let mut state = shared.state();
                    state.caps.learn_options(&response.config_options);
                    state.mapper.options_changed(&state.caps)
                };
                shared.apply(updates);
                shared.emit(Event::Capabilities);
            }
            Err(error) => shared.fail(SessionError::Agent(error.message)),
        });
        Ok(())
    }

    /// Signs in with `method`, one of the capabilities' methods the agent runs itself, and then opens the session.
    pub fn authenticate(&self, method: &str) -> Result<(), SessionError> {
        let connection = self.shared.connection().ok_or(SessionError::Closed)?;
        let shared = self.shared.clone();
        let later = connection.clone();
        // Signing in may wait for the user in a browser, so it has no deadline; opening the session after it has.
        ask(
            &connection,
            acp::AuthenticateRequest::new(method.to_string()),
            None,
            move |result| match result {
                Ok(_) => {
                    let opening = shared.clone();
                    let task = async move {
                        if let Err(error) = open(&opening, &later).await {
                            opening.fail(error);
                        }
                        Ok(())
                    };
                    if let Err(error) = shared.connection().map_or(Err(SessionError::Closed), |c| {
                        c.spawn(task).map_err(|e| SessionError::Agent(e.message))
                    }) {
                        shared.fail(error);
                    }
                }
                Err(error) => shared.fail(SessionError::Agent(error.message)),
            },
        );
        Ok(())
    }

    /// Ends the session: the connection closes, then the supervisor stops the process after `grace`.
    pub fn stop(&self, grace: Duration) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(shutdown) = self.shared.shutdown.lock_ignoring_poison().take() {
            let _ = shutdown.send(());
        }
        self.shared.supervisor.stop(self.shared.process, grace);
    }
}

/// Ends the session once its process is gone, should the agent's output stay open after it, such as through a child
/// the agent started.
fn watch(supervisor: &Supervisor, process: ProcessId, shared: &Weak<Shared>) {
    while supervisor.wait(process, Duration::from_secs(60)).is_none() {
        if shared.strong_count() == 0 {
            return;
        }
    }
    let deadline = Instant::now() + EXIT_GRACE;
    while Instant::now() < deadline {
        match shared.upgrade() {
            Some(shared) if !shared.ended.load(Ordering::SeqCst) => {}
            _ => return,
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(shared) = shared.upgrade()
        && let Some(shutdown) = shared.shutdown.lock_ignoring_poison().take()
    {
        let _ = shutdown.send(());
    }
}

/// Runs the connection over the agent's pipes until it closes or `closed` fires.
fn connect(shared: Arc<Shared>, stdin: ChildStdin, stdout: ChildStdout, closed: oneshot::Receiver<()>) {
    let transport = ByteStreams::new(
        blocking::Unblock::new(stdin),
        blocking::Unblock::new(Lines::new(stdout, LINE_LIMIT)),
    );
    let updates = shared.clone();
    let asks = shared.clone();
    let main = shared.clone();
    let result = futures::executor::block_on(
        // Unguarded, as the v1 guard turns an agent of another protocol version into an error that names the
        // version only in its text; `initialize` checks the version itself.
        Builder::new(Client)
            .name(term_base::identity().name.clone())
            .on_receive_notification(
                async move |notification: acp::SessionNotification, _cx| {
                    updates.on_update(notification);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: acp::RequestPermissionRequest, responder, _cx| {
                    asks.on_permission(request, responder);
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                *main.connection.lock_ignoring_poison() = Some(connection.clone());
                let serve = async {
                    match initialize(&main, &connection).await {
                        Ok(()) => match open(&main, &connection).await {
                            Ok(()) | Err(SessionError::SignIn) => {}
                            Err(error) => main.fail(error),
                        },
                        Err(error) => main.fail(error),
                    }
                    // A clean end of the agent's output does not end this future by itself (agent-client-protocol 5.4).
                    connection.incoming_closed().await;
                };
                futures::future::select(std::pin::pin!(serve), closed).await;
                Ok(())
            }),
    );
    shared.finish(result);
}

async fn initialize(shared: &Shared, connection: &ConnectionTo<Agent>) -> Result<(), SessionError> {
    let request = acp::InitializeRequest::new(ProtocolVersion::V1).client_info(acp::Implementation::new(
        term_base::identity().name.clone(),
        env!("CARGO_PKG_VERSION"),
    ));
    let response = within(INITIALIZE, connection.send_request(request).block_task())
        .await
        .ok_or(SessionError::Timeout("initialize"))?
        .map_err(|e| SessionError::Agent(e.message))?;
    let version = response.protocol_version.as_u16();
    shared.state().caps.learn_initialize(&response);
    shared.emit(Event::Capabilities);
    if version != ProtocolVersion::V1.as_u16() {
        return Err(SessionError::Version(version));
    }
    Ok(())
}

/// Opens the session: loads the one to continue where the agent can, else starts a new one.
async fn open(shared: &Shared, connection: &ConnectionTo<Agent>) -> Result<(), SessionError> {
    let launch = &shared.launch;
    let meta = launch.session_meta.as_ref().and_then(|m| m.as_object().cloned());
    let (can_load, servers) = {
        let state = shared.state();
        let servers = extras::handover(launch.handed_servers(), &state.caps, &launch.env, &launch.cwd);
        (state.caps.load_session, servers)
    };
    let opened = match launch.resume.clone().filter(|_| can_load) {
        Some(id) => {
            let request = acp::LoadSessionRequest::new(id.clone(), launch.cwd.clone())
                .mcp_servers(servers)
                .meta(meta);
            let session = acp::SessionId::new(id);
            shared.state().session = Some(session.clone());
            // The agent replays the whole conversation while it loads, so what the thread showed goes first.
            shared.apply(vec![Update::Clear]);
            within(LOAD_SESSION, connection.send_request(request).block_task())
                .await
                .map(|answer| answer.map(|r| (session, r.modes, r.config_options)))
                .ok_or("session/load")
        }
        None => {
            let request = acp::NewSessionRequest::new(launch.cwd.clone())
                .mcp_servers(servers)
                .meta(meta);
            within(NEW_SESSION, connection.send_request(request).block_task())
                .await
                .map(|answer| answer.map(|r| (r.session_id, r.modes, r.config_options)))
                .ok_or("session/new")
        }
    };
    let opened = match opened {
        Ok(answer) => answer,
        Err(method) => {
            shared.state().session = None;
            return Err(SessionError::Timeout(method));
        }
    };
    let (session, modes, options) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            shared.state().session = None;
            if error.code == acp::ErrorCode::AuthRequired {
                shared.fail(SessionError::SignIn);
                return Err(SessionError::SignIn);
            }
            return Err(SessionError::Agent(error.message));
        }
    };
    let updates = {
        let mut state = shared.state();
        let State { mapper, caps, .. } = &mut *state;
        caps.learn_modes(modes.as_ref());
        caps.learn_options(options.as_deref().unwrap_or_default());
        let mut updates = mapper.settle(now());
        updates.push(Update::Session(session.0.to_string()));
        if let Some(mode) = &caps.mode {
            updates.push(Update::Mode(mode.clone()));
        }
        updates.extend(mapper.options_changed(caps));
        updates.push(Update::Status(Status::Idle));
        state.session = Some(session);
        updates
    };
    shared.apply(updates);
    shared.emit(Event::Capabilities);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options() -> Vec<acp::PermissionOption> {
        serde_json::from_value(json!([
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
            {"optionId": "always", "name": "Always allow git", "kind": "allow_always"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
        ]))
        .unwrap()
    }

    #[test]
    fn a_request_without_an_answer_ends_at_its_deadline() {
        let silent = futures::future::pending::<()>();
        let start = Instant::now();
        assert_eq!(
            futures::executor::block_on(within(Duration::from_millis(20), silent)),
            None
        );
        assert!(start.elapsed() >= Duration::from_millis(20));
        let answered = futures::future::ready(7);
        assert_eq!(
            futures::executor::block_on(within(Duration::from_secs(5), answered)),
            Some(7)
        );
        assert_eq!(
            SessionError::Timeout("initialize").to_string(),
            "the agent did not answer initialize in time"
        );
    }

    #[test]
    fn an_overlong_line_of_the_agent_is_skipped_and_the_lines_after_it_pass_whole() {
        let input = b"{\"a\":1}\nxxxxxxxxxxxx\n{\"b\":22}\n{\"c\":3}".to_vec();
        let mut lines = Lines::new(&input[..], 9);
        let (mut out, mut buf) = (Vec::new(), [0u8; 3]);
        loop {
            let n = lines.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
        }
        assert_eq!(out, b"{\"a\":1}\n{\"b\":22}\n{\"c\":3}");
        assert_eq!(lines.read(&mut buf).unwrap(), 0);
        assert_eq!(lines.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn allowing_a_destructive_git_command_always_stores_no_rule() {
        let options = options();
        let id = |o: Option<&acp::PermissionOption>| o.map(|o| o.option_id.0.to_string());
        assert_eq!(id(picked(&options, Choice::Option(1), true)), Some("allow".into()));
        assert_eq!(id(picked(&options, Choice::Option(1), false)), Some("always".into()));
        assert_eq!(id(picked(&options, Choice::Option(2), true)), Some("reject".into()));
        assert_eq!(id(picked(&options, Choice::Dismiss, true)), None);
        let only_always: Vec<acp::PermissionOption> = options.into_iter().skip(1).collect();
        assert_eq!(id(picked(&only_always, Choice::Option(0), true)), None);
    }
}
