//! A session that plays a script instead of running an agent (F61), for the self-test and the tests: each prompt
//! plays the next turn of the script on a thread of its own, with messages, tool cards, requests that wait for an
//! answer, background work and files it writes in the session's folder, and ends the way the script says.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use term_thread::{
    AgentMessage, Answer, BackgroundItem, BackgroundKind, BackgroundPatch, Change, Decision, EntryKind, ErrorKind,
    Lane, Notice, OutputTail, PermissionOption, PermissionRequest, PlanApproval, Question, QuestionItem, RequestState,
    Status, TaskStatus, Thread, ToolCall, ToolKind, ToolPatch, ToolStatus, Update, UserMessage,
};

use crate::backend::PlanReply;

/// What a scripted session plays, turn by turn; a prompt past the last turn gets one that just succeeds.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// The turns, one per prompt in order.
    pub turns: Vec<ScriptTurn>,
    /// The pause before each step.
    pub pace: Duration,
}

impl Script {
    /// `turns` at a pace of a few milliseconds a step.
    pub fn new(turns: Vec<ScriptTurn>) -> Script {
        Script {
            turns,
            pace: Duration::from_millis(5),
        }
    }
}

/// One turn of a script.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptTurn {
    /// What happens, in order.
    pub steps: Vec<Step>,
    /// How the turn ends once the steps are played.
    pub end: End,
}

impl ScriptTurn {
    /// A turn of `steps` that ends as `end` says.
    pub fn new(steps: Vec<Step>, end: End) -> ScriptTurn {
        ScriptTurn { steps, end }
    }
}

/// One thing a scripted turn does.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// An answer of the agent.
    Say(String),
    /// An answer that streams: `text` appended `chunks` times, each an update of its own.
    Stream {
        /// What each chunk adds.
        text: String,
        /// How many chunks.
        chunks: usize,
    },
    /// A tool call that completes.
    Tool {
        /// The tool's name.
        name: String,
        /// Its input.
        input: Value,
    },
    /// Writes `contents` to `path`, relative to the session's folder, as an edit card.
    Write {
        /// The file, relative to the folder.
        path: PathBuf,
        /// What it holds afterwards.
        contents: String,
    },
    /// Asks to run a tool and waits for the answer; a refusal fails the call.
    Ask {
        /// The tool's name.
        tool: String,
        /// Its input.
        input: Value,
    },
    /// Asks the user a question and waits for the answer.
    Question(String),
    /// Asks to carry out a plan and waits for the answer; a rejection ends the turn as interrupted.
    Plan(String),
    /// Starts background work that runs until it is stopped.
    Background(String),
    /// Waits.
    Pause(Duration),
}

/// How a scripted turn ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// The agent finished.
    Succeed,
    /// The turn fails with this error.
    Fail(String),
    /// The turn runs until it is stopped.
    Hang,
    /// The process goes away with these last words.
    Crash(String),
}

/// What the session reports.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Changed(Vec<Change>),
    Ended { stderr: String },
}

#[derive(Debug)]
enum Reply {
    Permission { allow: bool, reason: Option<String> },
    Answers(Vec<Answer>),
    Plan(PlanReply),
}

struct State {
    thread: Arc<Thread>,
    played: usize,
    running: bool,
    cancelled: bool,
    ended: bool,
    /// The request the turn waits for, and the reply once it came.
    waiting: Option<(String, Option<Reply>)>,
    background: Vec<BackgroundItem>,
    next: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    sink: Box<dyn Fn(Event) + Send + Sync>,
    folder: PathBuf,
    script: Script,
}

/// A running scripted session; clones share it.
#[derive(Clone)]
pub(crate) struct Session {
    shared: Arc<Shared>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

fn put(key: &str, entry: EntryKind) -> Update {
    Update::Put {
        lane: Lane::Main,
        key: Some(key.into()),
        at: now(),
        entry,
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key(&self, prefix: &str) -> String {
        let mut state = self.state();
        state.next += 1;
        format!("{prefix}-{}", state.next)
    }

    fn apply(&self, updates: Vec<Update>) {
        let changes = Arc::make_mut(&mut self.state().thread).apply_all(updates);
        if !changes.is_empty() {
            (self.sink)(Event::Changed(changes));
        }
    }

    /// Whether the turn is to stop: cancelled, or the session ended.
    fn interrupted(&self) -> bool {
        let state = self.state();
        state.cancelled || state.ended
    }

    /// Shows the request `updates` make and waits for the reply to `request`; `None` when the turn was stopped
    /// first. The reply is awaited before anyone can see the request.
    fn ask_and_wait(&self, request: &str, updates: Vec<Update>) -> Option<Reply> {
        self.state().waiting = Some((request.to_string(), None));
        self.apply(updates);
        let mut state = self.state();
        loop {
            if state.cancelled || state.ended {
                state.waiting = None;
                return None;
            }
            if let Some((_, reply)) = state.waiting.as_mut()
                && let Some(reply) = reply.take()
            {
                state.waiting = None;
                return Some(reply);
            }
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Hands `reply` to the turn waiting for `request`.
    fn reply(&self, request: &str, reply: Reply) -> Result<(), String> {
        let mut state = self.state();
        match state.waiting.as_mut() {
            Some((waiting, slot)) if waiting == request && slot.is_none() => *slot = Some(reply),
            _ => return Err(format!("no request {request} waits for this answer")),
        }
        drop(state);
        self.changed.notify_all();
        Ok(())
    }

    /// Blocks until the turn is stopped.
    fn hang(&self) {
        let mut state = self.state();
        while !(state.cancelled || state.ended) {
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn finish(&self, end: &End) {
        let mut updates = Vec::new();
        let interrupted = self.interrupted();
        match end {
            _ if interrupted => updates.push(put(&self.key("notice"), EntryKind::Notice(Notice::Interrupted))),
            End::Succeed | End::Hang => {}
            End::Fail(message) => updates.push(put(
                &self.key("notice"),
                EntryKind::Notice(Notice::Error {
                    kind: ErrorKind::Turn,
                    message: message.clone(),
                }),
            )),
            End::Crash(message) => updates.push(put(
                &self.key("notice"),
                EntryKind::Notice(Notice::Error {
                    kind: ErrorKind::Process,
                    message: message.clone(),
                }),
            )),
        }
        updates.push(Update::Status(Status::Idle));
        let crashed = match end {
            End::Crash(message) if !interrupted => Some(message.clone()),
            _ => None,
        };
        // A prompt may follow as soon as the turn's end shows, so by then the session must take one, or have ended.
        let first_end = {
            let mut state = self.state();
            state.running = false;
            crashed.is_some() && !std::mem::replace(&mut state.ended, true)
        };
        self.apply(updates);
        if let Some(stderr) = crashed.filter(|_| first_end) {
            (self.sink)(Event::Ended { stderr });
        }
    }
}

/// Plays `turn` of the session behind `shared`.
fn play(shared: &Shared, turn: ScriptTurn) {
    for step in turn.steps {
        std::thread::sleep(shared.script.pace);
        if shared.interrupted() || !step_once(shared, step) {
            break;
        }
    }
    if matches!(turn.end, End::Hang) && !shared.interrupted() {
        shared.hang();
    }
    shared.finish(&turn.end);
}

/// Plays one step; false when the turn is to end here.
fn step_once(shared: &Shared, step: Step) -> bool {
    match step {
        Step::Say(text) => shared.apply(vec![put(&shared.key("message"), said(&text, false))]),
        Step::Stream { text, chunks } => {
            let key = shared.key("message");
            shared.apply(vec![put(&key, said("", true))]);
            for _ in 0..chunks {
                shared.apply(vec![Update::Append {
                    key: key.clone(),
                    text: text.clone(),
                }]);
            }
            shared.apply(vec![Update::Finish {
                key,
                text: None,
                at: now(),
            }]);
        }
        Step::Tool { name, input } => {
            let call = card(
                &shared.key("tool"),
                &name,
                ToolKind::Other,
                input,
                ToolStatus::Completed,
            );
            shared.apply(vec![put(&call.call_id.clone(), EntryKind::ToolCall(call))]);
        }
        Step::Write { path, contents } => write(shared, &path, &contents),
        Step::Ask { tool, input } => return ask(shared, &tool, input),
        Step::Question(text) => return question(shared, &text),
        Step::Plan(plan) => return plan_step(shared, &plan),
        Step::Background(label) => background(shared, label),
        Step::Pause(pause) => std::thread::sleep(pause),
    }
    true
}

fn said(text: &str, streaming: bool) -> EntryKind {
    EntryKind::AgentMessage(AgentMessage {
        markdown: text.into(),
        streaming,
    })
}

fn card(id: &str, name: &str, kind: ToolKind, input: Value, status: ToolStatus) -> ToolCall {
    let mut call = ToolCall::new(id, name, kind);
    call.input = input;
    call.status = status;
    call
}

fn write(shared: &Shared, path: &std::path::Path, contents: &str) {
    let full = shared.folder.join(path);
    let written = full
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&full, contents));
    let status = if written.is_ok() {
        ToolStatus::Completed
    } else {
        ToolStatus::Failed
    };
    let input = json!({"file_path": full.to_string_lossy(), "content": contents});
    let call = card(&shared.key("tool"), "Write", ToolKind::Edit, input, status);
    shared.apply(vec![put(&call.call_id.clone(), EntryKind::ToolCall(call))]);
}

fn tool_status(key: &str, status: ToolStatus) -> Update {
    Update::Tool {
        key: key.into(),
        patch: ToolPatch {
            status: Some(status),
            ended: Some(now()),
            ..ToolPatch::default()
        },
    }
}

fn ask(shared: &Shared, tool: &str, input: Value) -> bool {
    let call = shared.key("tool");
    let request = shared.key("request");
    let permission = PermissionRequest {
        request_id: request.clone(),
        tool_call: Some(call.clone()),
        tool_name: tool.into(),
        title: None,
        description: None,
        reason: None,
        blocked_path: None,
        input: input.clone(),
        options: vec![PermissionOption::AllowOnce, PermissionOption::Deny],
        default_to_no: false,
        state: RequestState::Pending,
    };
    let shown = vec![
        put(
            &call,
            EntryKind::ToolCall(card(&call, tool, ToolKind::Execute, input, ToolStatus::Pending)),
        ),
        put(&request, EntryKind::PermissionRequest(permission)),
        Update::Status(Status::RequiresAction),
    ];
    let Some(Reply::Permission { allow, reason }) = shared.ask_and_wait(&request, shown) else {
        shared.apply(vec![withdrawn(&request), tool_status(&call, ToolStatus::Failed)]);
        return false;
    };
    let decision = if allow {
        Decision::Allowed {
            always: false,
            edited: false,
        }
    } else {
        Decision::Denied { message: reason }
    };
    let status = if allow {
        ToolStatus::Completed
    } else {
        ToolStatus::Failed
    };
    shared.apply(vec![
        decided(&request, decision),
        tool_status(&call, status),
        Update::Status(Status::Running),
    ]);
    true
}

fn withdrawn(request: &str) -> Update {
    Update::Resolve {
        key: request.into(),
        state: RequestState::Withdrawn,
    }
}

fn decided(request: &str, decision: Decision) -> Update {
    Update::Resolve {
        key: request.into(),
        state: RequestState::Decided(decision),
    }
}

fn question(shared: &Shared, text: &str) -> bool {
    let request = shared.key("question");
    let entry = Question {
        request_id: Some(request.clone()),
        tool_call: shared.key("tool"),
        items: vec![QuestionItem {
            header: None,
            question: text.into(),
            options: Vec::new(),
            multi_select: false,
        }],
        state: RequestState::Pending,
    };
    let shown = vec![
        put(&request, EntryKind::Question(entry)),
        Update::Status(Status::RequiresAction),
    ];
    let Some(Reply::Answers(answers)) = shared.ask_and_wait(&request, shown) else {
        shared.apply(vec![withdrawn(&request)]);
        return false;
    };
    shared.apply(vec![
        decided(&request, Decision::Answered(answers)),
        Update::Status(Status::Running),
    ]);
    true
}

fn plan_step(shared: &Shared, plan: &str) -> bool {
    let request = shared.key("plan");
    let entry = PlanApproval {
        request_id: Some(request.clone()),
        tool_call: shared.key("tool"),
        plan: plan.into(),
        state: RequestState::Pending,
    };
    let shown = vec![
        put(&request, EntryKind::PlanApproval(entry)),
        Update::Status(Status::RequiresAction),
    ];
    let Some(Reply::Plan(reply)) = shared.ask_and_wait(&request, shown) else {
        shared.apply(vec![withdrawn(&request)]);
        return false;
    };
    let (decision, goes_on) = match reply {
        PlanReply::Approve => (Decision::Approved, true),
        PlanReply::Revise(feedback) => (Decision::Rejected { feedback, stop: false }, true),
        PlanReply::Reject => (
            Decision::Rejected {
                feedback: None,
                stop: true,
            },
            false,
        ),
    };
    shared.apply(vec![decided(&request, decision), Update::Status(Status::Running)]);
    if !goes_on {
        shared.state().cancelled = true;
    }
    goes_on
}

fn background(shared: &Shared, label: String) {
    let item = BackgroundItem {
        id: shared.key("task"),
        kind: BackgroundKind::Shell,
        label,
        status: TaskStatus::Running,
        started: Some(now()),
        ended: None,
        tool_call: None,
        parent: None,
        output: OutputTail::default(),
        is_backgrounded: true,
        ambient: false,
    };
    let running = {
        let mut state = shared.state();
        state.background.push(item);
        state.background.clone()
    };
    shared.apply(vec![Update::Background(running)]);
}

impl Session {
    /// A scripted session in `folder` that fills `thread` and reports to `sink`.
    pub(crate) fn start(
        script: Script,
        folder: PathBuf,
        thread: Thread,
        sink: impl Fn(Event) + Send + Sync + 'static,
    ) -> Session {
        let state = State {
            thread: Arc::new(thread),
            played: 0,
            running: false,
            cancelled: false,
            ended: false,
            waiting: None,
            background: Vec::new(),
            next: 0,
        };
        Session {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                changed: Condvar::new(),
                sink: Box::new(sink),
                folder,
                script,
            }),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<Thread> {
        Arc::clone(&self.shared.state().thread)
    }

    pub(crate) fn is_ended(&self) -> bool {
        self.shared.state().ended
    }

    /// Plays the next turn of the script for `message`.
    pub(crate) fn send(&self, message: UserMessage) -> Result<(), String> {
        let turn = {
            let mut state = self.shared.state();
            if state.ended {
                return Err("the session has ended".into());
            }
            if state.running {
                return Err("a turn is running".into());
            }
            state.running = true;
            state.cancelled = false;
            let turn = self.shared.script.turns.get(state.played).cloned();
            state.played += 1;
            turn.unwrap_or_else(|| ScriptTurn::new(Vec::new(), End::Succeed))
        };
        let title = term_thread::title_from(&message.text);
        let mut updates = vec![put(&self.shared.key("user"), EntryKind::UserMessage(message))];
        if self.snapshot().title().is_empty() && !title.is_empty() {
            updates.push(Update::Title(title));
        }
        updates.push(Update::Status(Status::Running));
        self.shared.apply(updates);
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("session-script".into())
            .spawn(move || play(&shared, turn));
        spawned.map(drop).map_err(|e| {
            self.shared.state().running = false;
            self.shared.apply(vec![Update::Status(Status::Idle)]);
            e.to_string()
        })
    }

    /// Stops the running turn at its next step.
    pub(crate) fn cancel(&self) {
        let running = {
            let mut state = self.shared.state();
            state.cancelled = state.running;
            state.running
        };
        if running {
            self.shared.changed.notify_all();
        }
    }

    /// Ends the session; its running turn stops.
    pub(crate) fn stop(&self) {
        let first = !std::mem::replace(&mut self.shared.state().ended, true);
        self.shared.changed.notify_all();
        if first {
            (self.shared.sink)(Event::Ended { stderr: String::new() });
        }
    }

    /// Stops the background item `task`; false when it does not run.
    pub(crate) fn stop_task(&self, task: &str) -> bool {
        let running = {
            let mut state = self.shared.state();
            let before = state.background.len();
            state.background.retain(|item| item.id != task);
            (state.background.len() < before).then(|| state.background.clone())
        };
        let Some(running) = running else { return false };
        self.shared.apply(vec![
            Update::BackgroundItem(BackgroundPatch {
                id: task.into(),
                status: Some(TaskStatus::Stopped),
                ended: Some(now()),
                ..BackgroundPatch::default()
            }),
            Update::Background(running),
        ]);
        true
    }

    pub(crate) fn answer(&self, request: &str, allow: bool, reason: Option<&str>) -> Result<(), String> {
        let reason = reason.map(str::to_string);
        self.shared.reply(request, Reply::Permission { allow, reason })
    }

    pub(crate) fn answer_questions(&self, request: &str, answers: Vec<Answer>) -> Result<(), String> {
        self.shared.reply(request, Reply::Answers(answers))
    }

    pub(crate) fn answer_plan(&self, request: &str, reply: PlanReply) -> Result<(), String> {
        self.shared.reply(request, Reply::Plan(reply))
    }
}
