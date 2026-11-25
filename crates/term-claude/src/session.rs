//! A running `claude` process: frames in, frames out, the thread it fills and the requests either side waits on.
//!
//! Everything that reads or writes the pipes runs on threads of its own; the methods only queue a line or take a
//! short lock, so the UI never waits for the CLI. Control requests the app sends expire on the app's timer thread
//! (`term_base::timer`) when the CLI does not answer in time.

mod dialogs;
mod error;
mod reply;
mod submission;

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use term_base::LockIgnoringPoison;

use serde_json::{Value, json};
use term_base::explain::Explain;
use term_process::framing::{self, Line};
use term_process::tail::{self, Tail};
use term_process::{Kind, ProcessId, Supervisor};
use term_thread::{
    Answer as QuestionAnswer, Change, Decision, EntryKind, ErrorKind, Lane, Notice, Thread, Update, UserMessage,
};

use crate::capabilities::Capabilities;
use crate::control::{
    self, Answer, Backgrounded, ContextCount, ContextUsage, ControlError, Incoming, InitializeResult, InterruptResult,
    PermissionAnswer, PermissionAsk, PlanAnswer, RemoteControlResult, Request, RewindResult, TaskOutput, UsageReport,
};
use crate::input;
use crate::launch::Launch;
use crate::mapping::{self, Mapper};
use crate::modes::{Effort, PermissionMode};
use crate::protocol::{self, ControlResponse, Message, Outcome, System};

pub use reply::Reply;

/// What the stderr tail keeps.
const STDERR_LIMIT: usize = 64 * 1024;

/// The longest line of the CLI's output that is read; a longer one is skipped, so that it costs neither the memory
/// nor the session. Frames with pictures stay well below it.
const LINE_LIMIT: usize = 64 * 1024 * 1024;

/// How much room for a line the reader keeps between lines.
const LINE_KEPT: usize = 1024 * 1024;

/// Words in the CLI's output that mean it has no usable sign-in.
const SIGN_IN_HINTS: [&str; 3] = ["Not logged in", "Please run /login", "Invalid API key"];

/// What a session tells the UI, on the session's reader thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The thread changed; read it with `Session::with_thread`.
    Changed(Vec<Change>),
    /// The CLI reported its commands, tools, models or account anew.
    Capabilities,
    /// The Remote Control connection changed; read it with `Session::remote_control`.
    RemoteControl,
    /// Something keeps the session from working, such as a missing sign-in.
    Failed(SessionError),
    /// The process ended; its last error output is attached.
    Ended {
        /// The end of what the CLI wrote to stderr.
        stderr: String,
    },
}

/// Where the session stands with Remote Control, which continues it on claude.ai and in the Claude apps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RemoteControl {
    /// The CLI does not offer it, or has not said yet.
    #[default]
    Unavailable,
    /// Not connected.
    Off,
    /// Connecting.
    Connecting,
    /// Connected; the session's page on claude.ai.
    Connected(String),
    /// The connection failed or broke off; why.
    Failed(String),
}

/// Why a session cannot do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The `claude` executable is not there.
    NotFound(PathBuf),
    /// The process did not start.
    Start(String),
    /// The CLI has no usable sign-in; the user signs in by running `claude` in a terminal, never through the app.
    SignInRequired(String),
    /// The CLI refused a guarded rewind (`Launch::rewind`) and ended; the same rewind fails again, so the session is
    /// resumed without `resume_at`, `resume_drops_turn` and `fork_session`.
    RewindRejected(String),
    /// The CLI has no request with this id waiting for an answer.
    UnknownRequest(String),
    /// The process has ended.
    Closed,
    /// The CLI stopped reading what the app writes to it.
    Congested,
}

/// How many lines may wait for the CLI to read them; more mean it hangs, and the app says so instead of piling up.
const WRITE_QUEUE: usize = 256;

type After = Box<dyn FnOnce(&Value, &mut State) -> Vec<Update> + Send>;
type Sink = Box<dyn Fn(Event) + Send + Sync>;

struct Pending {
    reply: reply::Answering,
    after: Option<After>,
}

struct State {
    mapper: Mapper,
    thread: Arc<Thread>,
    asks: HashMap<String, PermissionAsk>,
    capabilities: Capabilities,
    /// The CLI refused `get_context_usage`, so it is not asked after turns any more.
    context_refused: bool,
    remote: RemoteControl,
    /// The connection the latest `remote_control` answer opened, which `bridge_state` frames name.
    remote_epoch: Option<u64>,
    /// The page of the latest connection, kept for when it recovers.
    remote_url: Option<String>,
    /// Counts `remote_control` requests, so that only the latest answer counts.
    remote_asked: u64,
}

struct Shared {
    supervisor: Supervisor,
    process: ProcessId,
    writer: Mutex<Option<SyncSender<String>>>,
    state: Mutex<State>,
    pending: Mutex<HashMap<String, Pending>>,
    sink: Sink,
    next: AtomicU64,
    stderr: Arc<Mutex<Tail>>,
    stderr_reader: Mutex<Option<tail::Collecting>>,
    closed: AtomicBool,
    end_expected: AtomicBool,
    sign_in_reported: AtomicBool,
    /// Events that wait until the state lock is released.
    later: Mutex<Vec<Event>>,
    /// The longest any control request waits for its answer, in place of its own timeout.
    control_timeout: Option<Duration>,
}

/// A running Claude session; clones share it. It runs until `stop`, `close_input` or the supervisor ends it.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
}

impl Session {
    /// Starts `claude` as `launch` describes, filling `thread`, and sends `initialize`.
    pub fn start(
        supervisor: &Supervisor,
        launch: &Launch,
        thread: Thread,
        sink: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Session, SessionError> {
        if !launch.executable.is_file() {
            return Err(SessionError::NotFound(launch.executable.clone()));
        }
        let spawned = supervisor
            .spawn(launch.command(), Kind::Agent, "claude")
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => SessionError::NotFound(launch.executable.clone()),
                _ => SessionError::Start(e.to_string()),
            })?;
        let (Some(stdin), Some(stdout), Some(stderr)) = (spawned.stdin, spawned.stdout, spawned.stderr) else {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start("the pipes are missing".into()));
        };
        let (tx, rx) = sync_channel(WRITE_QUEUE);
        let stderr_tail = Arc::new(Mutex::new(Tail::new(STDERR_LIMIT)));
        let shared = Arc::new(Shared {
            supervisor: supervisor.clone(),
            process: spawned.id,
            writer: Mutex::new(Some(tx)),
            state: Mutex::new(State::new(launch, thread)),
            pending: Mutex::new(HashMap::new()),
            sink: Box::new(sink),
            next: AtomicU64::new(1),
            stderr: stderr_tail.clone(),
            stderr_reader: Mutex::new(None),
            closed: AtomicBool::new(false),
            end_expected: AtomicBool::new(false),
            sign_in_reported: AtomicBool::new(false),
            later: Mutex::new(Vec::new()),
            control_timeout: launch.control_timeout,
        });
        let weak = Arc::downgrade(&shared);
        supervisor.on_stop(spawned.id, move || {
            if let Some(shared) = weak.upgrade() {
                shared.close_writer();
            }
        });
        let started = spawn_threads(&shared, stdin, rx, stdout, stderr, stderr_tail);
        if let Err(e) = started {
            supervisor.stop(spawned.id, Duration::from_millis(500));
            return Err(SessionError::Start(e.to_string()));
        }
        let session = Session { shared };
        session.initialize(launch);
        if launch.resume.is_some() {
            drop(session.usage());
        }
        Ok(session)
    }

    fn initialize(&self, launch: &Launch) {
        let request = Request::Initialize {
            per_task_stop: true,
            progress_summaries: launch.progress_summaries,
            forward_subagent_text: launch.forward_subagent_text,
        };
        let weak = Arc::downgrade(&self.shared);
        let remote = launch.remote_control;
        let reply: Reply<InitializeResult> = self.request(
            request,
            Some(Box::new(move |payload, state| {
                let Ok(result) = InitializeResult::read(payload.clone()) else {
                    return Vec::new();
                };
                state.capabilities.learn_initialize(&result);
                if let Some(shared) = weak.upgrade() {
                    shared.emit_later(Event::Capabilities);
                    if result.signed_out() {
                        shared.sign_in_required("Not logged in".into());
                    }
                    if result.remote_control_offered() {
                        state.remote = RemoteControl::Off;
                        if remote.unwrap_or_else(|| result.remote_control_at_start()) {
                            shared.ask_remote(state, true);
                        }
                        shared.emit_later(Event::RemoteControl);
                    }
                }
                vec![result.billing()]
            })),
        );
        let weak = Arc::downgrade(&self.shared);
        reply.then(move |result| {
            if let (Err(ControlError::Timeout), Some(shared)) = (result, weak.upgrade()) {
                shared.emit(Event::Failed(SessionError::Start(ControlError::Timeout.explain())));
            }
        });
    }

    /// Sends `message` as a new prompt and returns the CLI's id for it, which rewinding names.
    pub fn send(&self, message: UserMessage) -> Result<String, SessionError> {
        if self.shared.closed.load(Ordering::SeqCst) {
            return Err(SessionError::Closed);
        }
        let uuid = input::uuid();
        let line = input::user_frame(&message, &uuid).to_string();
        let changes = self
            .shared
            .lock_state()
            .submit(message, &uuid, || self.shared.write_line(line))?;
        self.shared.emit_changes(changes);
        Ok(uuid)
    }

    /// Answers the permission request `request_id`.
    pub fn answer(&self, request_id: &str, answer: PermissionAnswer) -> Result<(), SessionError> {
        self.settle(request_id, |ask| {
            let answer = answer.offered_by(ask);
            let decision = match &answer {
                PermissionAnswer::Allow { updated_input, always } => Decision::Allowed {
                    always: !always.is_empty(),
                    edited: updated_input.as_ref().is_some_and(|input| *input != ask.input),
                },
                PermissionAnswer::Deny { message, .. } => Decision::Denied {
                    message: Some(message.clone()),
                },
            };
            (answer.payload(ask), decision)
        })
    }

    /// Answers the AskUserQuestion request `request_id`; several labels of one question are joined by a comma.
    pub fn answer_questions(&self, request_id: &str, answers: Vec<QuestionAnswer>) -> Result<(), SessionError> {
        self.settle(request_id, |ask| {
            let pairs: Vec<(String, String)> = answers.iter().map(|a| (a.question.clone(), a.answer.clone())).collect();
            let allow = PermissionAnswer::Allow {
                updated_input: Some(control::question_answer(&ask.input, &pairs)),
                always: Vec::new(),
            };
            (allow.payload(ask), Decision::Answered(answers.clone()))
        })
    }

    /// Answers the plan of the ExitPlanMode request `request_id`.
    pub fn answer_plan(&self, request_id: &str, answer: PlanAnswer) -> Result<(), SessionError> {
        self.settle(request_id, |ask| (answer.permission().payload(ask), answer.decision()))
    }

    fn settle(
        &self,
        request_id: &str,
        f: impl FnOnce(&PermissionAsk) -> (Value, Decision),
    ) -> Result<(), SessionError> {
        let changes = {
            let mut state = self.shared.lock_state();
            let Some(ask) = state.asks.get(request_id) else {
                return Err(SessionError::UnknownRequest(request_id.to_string()));
            };
            let (payload, decision) = f(ask);
            self.shared.write(&control::success_frame(request_id, payload))?;
            state.asks.remove(request_id);
            let updates = state.mapper.decided(request_id, decision);
            Arc::make_mut(&mut state.thread).apply_all(updates)
        };
        self.shared.emit_changes(changes);
        Ok(())
    }

    /// Stops the current turn; background tasks keep running.
    pub fn interrupt(&self) -> Reply<InterruptResult> {
        self.request(Request::Interrupt, None)
    }

    /// Changes the permission mode.
    pub fn set_permission_mode(&self, mode: PermissionMode) -> Reply<()> {
        let name = mode.as_str().to_string();
        self.request(
            Request::SetPermissionMode(mode),
            Some(Box::new(move |_, _| vec![Update::Mode(name)])),
        )
    }

    /// Changes the model for the following turns; `None` returns to the default.
    pub fn set_model(&self, model: Option<&str>) -> Reply<()> {
        let update = model.map(|m| Update::Model(m.to_string()));
        self.request(
            Request::SetModel(model.map(str::to_string)),
            Some(Box::new(move |_, _| update.into_iter().collect())),
        )
    }

    /// Changes the reasoning effort of the running session.
    pub fn set_effort(&self, level: Effort) -> Reply<()> {
        let name = level.as_str().to_string();
        self.request(
            Request::SetEffort(level),
            Some(Box::new(move |_, _| vec![Update::Effort(Some(name))])),
        )
    }

    /// Stops one background task or subagent.
    pub fn stop_task(&self, task_id: &str) -> Reply<()> {
        self.request(Request::StopTask(task_id.to_string()), None)
    }

    /// Moves the foreground task of `tool_use_id`, or all of them, to the background; tells whether any moved.
    pub fn background(&self, tool_use_id: Option<&str>) -> Reply<Backgrounded> {
        self.request(Request::BackgroundTasks(tool_use_id.map(str::to_string)), None)
    }

    /// Where the session stands with Remote Control.
    pub fn remote_control(&self) -> RemoteControl {
        self.shared.lock_state().remote.clone()
    }

    /// Connects the session to Remote Control or ends that, when the CLI offers it.
    pub fn set_remote_control(&self, on: bool) {
        {
            let mut state = self.shared.lock_state();
            if state.remote == RemoteControl::Unavailable {
                return;
            }
            self.shared.ask_remote(&mut state, on);
        }
        self.shared.emit(Event::RemoteControl);
    }

    /// Reads the end of a shell or monitor task's output and keeps it on the thread's background item.
    pub fn task_output(&self, task_id: &str) -> Reply<TaskOutput> {
        let task = task_id.to_string();
        self.request(
            Request::GetTaskOutput(task_id.to_string()),
            Some(Box::new(move |payload, state| {
                match TaskOutput::read(payload.clone()) {
                    Ok(output) => state.mapper.task_output(&task, &output),
                    Err(_) => Vec::new(),
                }
            })),
        )
    }

    /// Reads what fills the context window, counted as `count` says, and keeps it on the thread as its breakdown.
    /// The session asks for an estimate after every turn itself.
    pub fn context_usage(&self, count: ContextCount) -> Reply<ContextUsage> {
        self.request(Request::GetContextUsage { count }, Some(context_after(count)))
    }

    /// Reads the session's cost and the plan's usage windows; experimental in the CLI. The session's totals in the
    /// answer tell a resumed session what its next turn costs.
    pub fn usage(&self) -> Reply<UsageReport> {
        let prompts = self.shared.lock_state().mapper.prompts();
        self.request(
            Request::GetUsage,
            Some(Box::new(move |payload, state| {
                if let Ok(report) = <UsageReport as Answer>::read(payload.clone()) {
                    state.mapper.baseline(&report, prompts);
                }
                Vec::new()
            })),
        )
    }

    /// Restores the files the file tools changed since the user message `user_message_id`; experimental in the CLI
    /// and only with `Launch::file_checkpoints`.
    pub fn rewind_files(&self, user_message_id: &str) -> Reply<RewindResult> {
        self.rewind(user_message_id, false)
    }

    /// What [`Session::rewind_files`] would restore, without restoring it.
    pub fn preview_rewind(&self, user_message_id: &str) -> Reply<RewindResult> {
        self.rewind(user_message_id, true)
    }

    fn rewind(&self, user_message_id: &str, dry_run: bool) -> Reply<RewindResult> {
        let user_message_id = user_message_id.to_string();
        self.request(
            Request::RewindFiles {
                user_message_id,
                dry_run,
            },
            None,
        )
    }

    fn request<T: Answer>(&self, request: Request, after: Option<After>) -> Reply<T> {
        self.shared.request(request, after)
    }

    /// Calls `f` with the thread as it is now.
    pub fn with_thread<R>(&self, f: impl FnOnce(&Thread) -> R) -> R {
        f(&self.shared.lock_state().thread)
    }

    /// A copy of the thread as it is now; entries are shared, not copied.
    pub fn snapshot(&self) -> Thread {
        Thread::clone(&self.shared.lock_state().thread)
    }

    /// The thread as it is now, shared with the session, which copies it before its next change.
    pub fn shared_snapshot(&self) -> Arc<Thread> {
        Arc::clone(&self.shared.lock_state().thread)
    }

    /// What the CLI reported about itself so far.
    pub fn capabilities(&self) -> Capabilities {
        self.shared.lock_state().capabilities.clone()
    }

    /// Whether the process has ended.
    pub fn is_ended(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// The supervisor's handle for this CLI process.
    pub fn process_id(&self) -> ProcessId {
        self.shared.process
    }

    /// Whether the latest state allows closing the CLI and resuming later.
    pub fn can_park(&self) -> bool {
        let state = self.shared.lock_state();
        !self.is_ended()
            && state.thread.session().is_some()
            && state.thread.status() == term_thread::Status::Idle
            && state.asks.is_empty()
            && state.thread.pending_requests().is_empty()
            && matches!(state.remote, RemoteControl::Unavailable | RemoteControl::Off)
            && state.thread.subagents().iter().all(|task| task.status.is_final())
            && state.thread.background().iter().all(|task| task.status.is_final())
    }

    /// Checks for work in the live process tree, including commands Claude has not reported.
    pub fn has_process_work(&self) -> bool {
        self.shared.supervisor.agent_has_work(self.shared.process)
    }

    /// Closes the CLI's input; it finishes what it has and ends.
    pub fn close_input(&self) {
        self.shared.close_writer();
    }

    /// Ends the process in the background: input closed, then SIGTERM and SIGKILL to its group after `grace` each.
    pub fn stop(&self, grace: Duration) {
        self.shared.end_expected.store(true, Ordering::SeqCst);
        self.shared.supervisor.stop(self.shared.process, grace);
    }
}

fn spawn_threads(
    shared: &Arc<Shared>,
    stdin: ChildStdin,
    rx: Receiver<String>,
    stdout: ChildStdout,
    stderr: impl Read + Send + 'static,
    stderr_tail: Arc<Mutex<Tail>>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("claude-writer".into())
        .spawn(move || {
            term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
            write_lines(stdin, &rx);
        })?;
    let stderr_reader = tail::collect(stderr, stderr_tail)?;
    *shared.stderr_reader.lock_ignoring_poison() = Some(stderr_reader);
    let reader = shared.clone();
    std::thread::Builder::new()
        .name("claude-reader".into())
        .spawn(move || {
            term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
            reader.read(stdout);
        })?;
    Ok(())
}

fn write_lines(mut stdin: ChildStdin, rx: &Receiver<String>) {
    while let Ok(line) = rx.recv() {
        let written = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush());
        if let Err(error) = written {
            if error.kind() != std::io::ErrorKind::BrokenPipe {
                term_base::diag::report("claude: writing to the CLI", error);
            }
            break;
        }
    }
}

/// The mapper for a session `launch` starts; a resumed session's totals are not known yet.
fn mapper_for(launch: &Launch) -> Mapper {
    let mut mapper = Mapper::new();
    if launch.resume.is_some() {
        mapper.resumed();
    }
    mapper
}

/// Keeps a `get_context_usage` answer on the thread.
fn context_after(count: ContextCount) -> After {
    Box::new(move |payload, state| match ContextUsage::read(payload.clone()) {
        Ok(usage) => state.mapper.context(&usage, count, SystemTime::now()),
        Err(_) => Vec::new(),
    })
}

impl Shared {
    /// Sends `request`; its answer, the refusal, the timeout or the end of the session reaches the reply once.
    fn request<T: Answer>(self: &Arc<Self>, request: Request, after: Option<After>) -> Reply<T> {
        let id = format!("req-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (answering, reply) = reply::pair();
        if self.closed.load(Ordering::SeqCst) {
            answering.answer(Err(ControlError::Closed));
            return reply;
        }
        self.lock_pending().insert(
            id.clone(),
            Pending {
                reply: answering,
                after,
            },
        );
        if let Err(error) = self.write(&request.frame(&id)) {
            let pending = self.lock_pending().remove(&id);
            if let Some(pending) = pending {
                pending.reply.answer(Err(match error {
                    SessionError::Congested => ControlError::Congested,
                    _ => ControlError::Closed,
                }));
            }
            return reply;
        }
        let timeout = self
            .control_timeout
            .map_or(request.timeout(), |most| most.min(request.timeout()));
        let shared = Arc::downgrade(self);
        term_base::timer::after(timeout, move || {
            if let Some(shared) = shared.upgrade() {
                shared.expire(&id);
            }
        });
        reply
    }

    /// Asks after a turn how full the context window is, unless the CLI refused before or the session ends.
    fn ask_context(self: &Arc<Self>) {
        if self.lock_state().context_refused
            || self.closed.load(Ordering::SeqCst)
            || self.end_expected.load(Ordering::SeqCst)
        {
            return;
        }
        let count = ContextCount::Estimated;
        let reply: Reply<ContextUsage> = self.request(Request::GetContextUsage { count }, Some(context_after(count)));
        let weak = Arc::downgrade(self);
        reply.then(move |result| {
            if let (Err(ControlError::Refused(_)), Some(shared)) = (result, weak.upgrade()) {
                shared.lock_state().context_refused = true;
            }
        });
    }

    /// Sends `remote_control`; the answer moves `state.remote` on in the order the CLI wrote it, unless a later
    /// request overtook it.
    fn ask_remote(self: &Arc<Self>, state: &mut State, on: bool) {
        state.remote_asked += 1;
        let asked = state.remote_asked;
        state.remote = if on {
            RemoteControl::Connecting
        } else {
            RemoteControl::Off
        };
        let weak = Arc::downgrade(self);
        let after: After = Box::new(move |payload, state| {
            if state.remote_asked != asked || state.remote == RemoteControl::Unavailable || !on {
                return Vec::new();
            }
            state.remote = match RemoteControlResult::read(payload.clone()) {
                Ok(RemoteControlResult {
                    session_url: Some(url),
                    bridge_epoch,
                }) => {
                    state.remote_epoch = bridge_epoch;
                    state.remote_url = Some(url.clone());
                    RemoteControl::Connected(url)
                }
                Ok(_) => RemoteControl::Failed("no session page".into()),
                Err(error) => RemoteControl::Failed(error.to_string()),
            };
            if let Some(shared) = weak.upgrade() {
                shared.emit_later(Event::RemoteControl);
            }
            Vec::new()
        });
        let reply: Reply<RemoteControlResult> = self.request(Request::RemoteControl(on), Some(after));
        let weak = Arc::downgrade(self);
        reply.then(move |result| {
            let (Err(error), Some(shared)) = (result, weak.upgrade()) else {
                return;
            };
            {
                let mut state = shared.lock_state();
                if state.remote_asked != asked || state.remote == RemoteControl::Unavailable {
                    return;
                }
                state.remote = RemoteControl::Failed(error.to_string());
            }
            shared.emit(Event::RemoteControl);
        });
    }

    /// Follows a `bridge_state` frame: a connection that breaks off is failed, one that recovers connected again.
    fn bridge_state(&self, state: &mut State, frame: &str, epoch: Option<u64>, detail: Option<&String>) {
        let current = epoch.is_none() || epoch == state.remote_epoch;
        let next = match (&state.remote, frame) {
            (RemoteControl::Connected(_), "failed" | "policy_disabled") if current => {
                RemoteControl::Failed(detail.cloned().unwrap_or_else(|| frame.to_string()))
            }
            (RemoteControl::Failed(_), "connected") if current => match &state.remote_url {
                Some(url) => RemoteControl::Connected(url.clone()),
                None => return,
            },
            _ => return,
        };
        state.remote = next;
        self.emit_later(Event::RemoteControl);
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state.lock_ignoring_poison()
    }

    fn lock_pending(&self) -> MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock_ignoring_poison()
    }

    fn lock_writer(&self) -> MutexGuard<'_, Option<SyncSender<String>>> {
        self.writer.lock_ignoring_poison()
    }

    fn write(&self, frame: &Value) -> Result<(), SessionError> {
        self.write_line(frame.to_string())
    }

    fn close_writer(&self) {
        self.end_expected.store(true, Ordering::SeqCst);
        self.lock_writer().take();
    }

    fn write_line(&self, line: String) -> Result<(), SessionError> {
        match self.lock_writer().as_ref() {
            Some(writer) => writer.try_send(line).map_err(|error| match error {
                TrySendError::Full(_) => SessionError::Congested,
                TrySendError::Disconnected(_) => SessionError::Closed,
            }),
            None => Err(SessionError::Closed),
        }
    }

    fn emit(&self, event: Event) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.sink)(event)));
    }

    fn emit_changes(&self, changes: Vec<Change>) {
        if !changes.is_empty() {
            self.emit(Event::Changed(changes));
        }
    }

    /// Queues `event` for after the state lock is released.
    fn emit_later(&self, event: Event) {
        self.later.lock_ignoring_poison().push(event);
    }

    fn flush_later(&self) {
        let events = std::mem::take(&mut *self.later.lock_ignoring_poison());
        for event in events {
            self.emit(event);
        }
    }

    fn sign_in_required(&self, why: String) {
        if !self.sign_in_reported.swap(true, Ordering::SeqCst) {
            self.emit_later(Event::Failed(SessionError::SignInRequired(why)));
        }
    }

    /// Withdraws the request `id` when it still waits for its answer, and tells its reply so.
    fn expire(&self, id: &str) {
        let pending = self.lock_pending().remove(id);
        if let Some(pending) = pending {
            let _ = self.write(&control::cancel_frame(id));
            pending.reply.answer(Err(ControlError::Timeout));
        }
    }

    fn read(self: &Arc<Self>, stdout: ChildStdout) {
        let mut reader = BufReader::with_capacity(256 * 1024, stdout);
        let mut line = Vec::new();
        loop {
            // One frame with large pictures would otherwise keep its size reserved for the session's life.
            if line.capacity() > LINE_KEPT {
                line = Vec::new();
            }
            match framing::read_line(&mut reader, &mut line, LINE_LIMIT) {
                Ok(Line::End) => break,
                Err(error) => {
                    term_base::diag::report("claude: reading from the CLI", error);
                    break;
                }
                Ok(Line::TooLong) => term_base::diag::report(
                    "claude: a line of the CLI was skipped",
                    format_args!("it is longer than {} MiB", LINE_LIMIT >> 20),
                ),
                Ok(Line::Read) => {
                    let text = String::from_utf8_lossy(&line);
                    match protocol::parse(text.trim_end()) {
                        Ok(Message::Malformed(frame)) => {
                            let kind = frame.get("type").and_then(Value::as_str).unwrap_or_default();
                            term_base::diag::report("claude: a frame that does not fit its type", kind);
                            self.handle(Message::Malformed(frame));
                        }
                        Ok(message) => self.handle(message),
                        Err(error) => term_base::diag::report("claude: a line that is not a frame", error.0),
                    }
                    self.flush_later();
                }
            }
        }
        self.finish();
    }

    /// Waits briefly for the stderr reader, which may still drain the pipe when stdout ends; a grandchild that
    /// keeps stderr open must not hold the session's end.
    fn await_stderr(&self) {
        if let Some(reader) = self.stderr_reader.lock_ignoring_poison().take() {
            reader.wait(Duration::from_millis(500));
        }
    }

    fn handle(self: &Arc<Self>, message: Message) {
        match message {
            Message::ControlResponse(response) => self.answered(response),
            Message::ControlRequest { request_id, request } => self.incoming(&request_id, &request),
            Message::ControlCancel { request_id } => {
                let changes = {
                    let mut state = self.lock_state();
                    state.asks.remove(&request_id);
                    let updates = state.mapper.withdrawn(&request_id);
                    Arc::make_mut(&mut state.thread).apply_all(updates)
                };
                self.emit_changes(changes);
            }
            other => {
                let now = SystemTime::now();
                let changes = {
                    let mut state = self.lock_state();
                    match &other {
                        Message::System(System::Init(init)) => {
                            state.capabilities.learn_init(init);
                            self.emit_later(Event::Capabilities);
                        }
                        Message::System(System::BridgeState {
                            state: frame,
                            epoch,
                            detail,
                        }) => {
                            self.bridge_state(&mut state, frame, *epoch, detail.as_ref());
                        }
                        Message::System(System::CommandsChanged(commands)) => {
                            state.capabilities.slash_commands = commands
                                .iter()
                                .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_string))
                                .collect();
                            self.emit_later(Event::Capabilities);
                        }
                        Message::Assistant(a) => {
                            if let Some(why) = mapping::sign_in_problem(a) {
                                self.sign_in_required(why);
                            }
                        }
                        Message::Result(r) => {
                            if let Some(why) = mapping::rewind_rejected(r) {
                                self.end_expected.store(true, Ordering::SeqCst);
                                self.emit_later(Event::Failed(SessionError::RewindRejected(why)));
                            }
                        }
                        _ => {}
                    }
                    let updates = state.mapper.map(&other, now);
                    Arc::make_mut(&mut state.thread).apply_all(updates)
                };
                self.emit_changes(changes);
                if matches!(other, Message::Result(_)) {
                    self.ask_context();
                }
            }
        }
    }

    fn answered(&self, response: ControlResponse) {
        let Some(pending) = self.lock_pending().remove(&response.request_id) else {
            return;
        };
        if response.subtype != Outcome::Success {
            let why = response.error.unwrap_or_else(|| response.subtype.as_str().to_string());
            pending.reply.answer(Err(ControlError::Refused(why)));
            return;
        }
        let payload = response.response.unwrap_or_else(|| json!({}));
        if let Some(after) = pending.after {
            let changes = {
                let mut state = self.lock_state();
                let updates = after(&payload, &mut state);
                Arc::make_mut(&mut state.thread).apply_all(updates)
            };
            self.emit_changes(changes);
        }
        pending.reply.answer(Ok(payload));
    }

    fn incoming(&self, request_id: &str, body: &Value) {
        match Incoming::read(body) {
            Incoming::CanUseTool(ask) => {
                let changes = {
                    let mut state = self.lock_state();
                    let updates = state.mapper.permission(request_id, &ask, SystemTime::now());
                    state.asks.insert(request_id.to_string(), *ask);
                    Arc::make_mut(&mut state.thread).apply_all(updates)
                };
                self.emit_changes(changes);
            }
            Incoming::HookCallback { callback_id, input } => {
                let frame = match crate::hooks::hook_answer(&callback_id, &input) {
                    Some(answer) => control::success_frame(request_id, answer),
                    None => control::error_frame(request_id, &format!("unknown hook: {callback_id}")),
                };
                let _ = self.write(&frame);
            }
            // Requests the app does not serve, such as dialogs, are refused rather than allowed.
            Incoming::Other { subtype } => {
                let _ = self.write(&control::error_frame(
                    request_id,
                    &format!("unsupported control request: {subtype}"),
                ));
            }
        }
    }

    fn finish(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.lock_writer().take();
        let pending: Vec<Pending> = self.lock_pending().drain().map(|(_, pending)| pending).collect();
        for pending in pending {
            pending.reply.answer(Err(ControlError::Closed));
        }
        self.await_stderr();
        let stderr = self.stderr.lock_ignoring_poison().text();
        let changes = {
            let mut state = self.lock_state();
            state.asks.clear();
            state.remote = RemoteControl::Unavailable;
            let now = SystemTime::now();
            let mut updates = state.mapper.ended(now);
            if !self.end_expected.load(Ordering::SeqCst) {
                let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
                updates.push(Update::Put {
                    lane: Lane::Main,
                    key: None,
                    at: now,
                    entry: EntryKind::Notice(Notice::Error {
                        kind: ErrorKind::Process,
                        message: last.trim().to_string(),
                    }),
                });
            }
            Arc::make_mut(&mut state.thread).apply_all(updates)
        };
        self.emit_changes(changes);
        if let Some(hint) = SIGN_IN_HINTS.iter().find(|hint| stderr.contains(*hint)) {
            self.sign_in_required((*hint).to_string());
        }
        self.flush_later();
        self.emit(Event::Ended { stderr });
    }
}
