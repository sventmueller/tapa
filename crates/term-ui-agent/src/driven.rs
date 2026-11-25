//! An agent tab's own session in the registry: the tab subscribes to the session's feed, whose first batch is the
//! snapshot, hands the user's messages to the driver and answers requests through it; the session ends with the
//! tab.

use std::rc::Rc;

use term_agents::{
    AnswerError, Batch, Driver, Launch, LaunchError, PathKind, PlanReply, Prompt, SessionId, SubscriberId, TurnId,
};
use term_base::explain::Explained;
use term_thread::{Answer, Thread, UserMessage};

/// The registry as an agent tab reaches it, by the session's id.
pub(crate) trait Sessions {
    /// Runs `f` with the driver of `session`; false, without running it, once the registry let the session go.
    fn with_driver(&self, session: SessionId, f: &mut dyn FnMut(&mut Driver)) -> bool;
    /// Starts the session `launch` describes, whose requests all go to the user.
    fn start(&self, launch: Launch) -> Result<SessionId, LaunchError>;
    /// Ends `session` and lets it go.
    fn end(&self, session: SessionId);
}

/// An agent tab's subscription to its session in the registry.
#[derive(Clone)]
pub(crate) struct Driven {
    session: SessionId,
    subscriber: SubscriberId,
    sessions: Rc<dyn Sessions>,
    /// The path the session takes.
    kind: PathKind,
}

impl Driven {
    /// Starts the tab's own session `launch` describes in `sessions` and subscribes to it; with the first batch, the
    /// snapshot.
    pub(crate) fn start(sessions: &Rc<dyn Sessions>, launch: Launch) -> Result<(Driven, Option<Batch>), LaunchError> {
        let session = sessions.start(launch)?;
        let Some((subscriber, kind)) = subscribe(sessions.as_ref(), session) else {
            sessions.end(session);
            return Err(LaunchError::Start(String::new()));
        };
        let driven = Driven {
            session,
            subscriber,
            sessions: sessions.clone(),
            kind,
        };
        let first = driven.take();
        Ok((driven, first))
    }

    /// What `f` gives of the session's driver, while the registry holds it.
    fn with<R>(&self, f: impl FnOnce(&mut Driver) -> R) -> Option<R> {
        let mut f = Some(f);
        let mut out = None;
        self.sessions.with_driver(self.session, &mut |driver| {
            if let Some(f) = f.take() {
                out = Some(f(driver));
            }
        });
        out
    }

    /// The registry's id of the session.
    pub(crate) fn session(&self) -> SessionId {
        self.session
    }

    /// The path the session takes.
    pub(crate) fn kind(&self) -> PathKind {
        self.kind
    }

    /// What waits in the tab's feed; `None` once the session is gone.
    pub(crate) fn take(&self) -> Option<Batch> {
        self.with(|driver| driver.take(self.subscriber)).flatten()
    }

    /// The thread as of the driver's last flush.
    pub(crate) fn snapshot(&self) -> Option<Thread> {
        self.with(|driver| driver.thread().clone())
    }

    /// Whether the session takes prompts: while its agent runs.
    pub(crate) fn is_live(&self) -> bool {
        self.with(|driver| driver.takes_prompts()).unwrap_or(false)
    }

    /// Whether a recording plays instead of an agent.
    pub(crate) fn is_replay(&self) -> bool {
        self.with(|driver| driver.is_replay()).unwrap_or(false)
    }

    /// What Claude Code offers in the session, such as its models.
    pub(crate) fn capabilities(&self) -> Option<term_claude::Capabilities> {
        self.with(|driver| driver.claude_capabilities()).flatten()
    }

    /// The Claude session behind the driver, for what only Claude Code does.
    pub(crate) fn claude(&self) -> Option<term_claude::Session> {
        self.with(|driver| driver.claude().cloned()).flatten()
    }

    pub(crate) fn parkable_claude(&self) -> Option<term_claude::Session> {
        self.with(|driver| {
            if !driver.is_ready() || driver.is_working() {
                return None;
            }
            driver.claude().filter(|session| session.can_park()).cloned()
        })
        .flatten()
    }

    /// The Codex session behind the driver, for what only Codex does.
    pub(crate) fn codex(&self) -> Option<term_codex::Session> {
        self.with(|driver| driver.codex().cloned()).flatten()
    }

    /// The OpenCode session behind the driver, for what only OpenCode does.
    pub(crate) fn opencode(&self) -> Option<term_opencode::Session> {
        self.with(|driver| driver.opencode().cloned()).flatten()
    }

    /// The ACP session behind the driver, for what only an ACP agent does.
    pub(crate) fn acp(&self) -> Option<term_acp::Session> {
        self.with(|driver| driver.acp().cloned()).flatten()
    }

    /// Hands `prompt` to the driver, which sends it once no turn runs, after a checkpoint with its buffers; the turn
    /// it begins.
    pub(crate) fn prompt(&self, prompt: Prompt) -> Result<TurnId, Explained> {
        match self.with(|driver| driver.send(prompt)) {
            Some(Ok(turn)) => Ok(turn),
            Some(Err(error)) => Err(Explained(error.to_string())),
            None => Err(gone()),
        }
    }

    /// Hands `message` to the turn that runs, at once; the agent's id of it.
    pub(crate) fn steer(&self, message: UserMessage) -> Result<Option<String>, Explained> {
        match self.with(|driver| driver.steer(message)) {
            Some(Ok(id)) => Ok(id),
            Some(Err(error)) => Err(Explained(error.to_string())),
            None => Err(gone()),
        }
    }

    /// Stops the turn that runs; the station's work goes on.
    pub(crate) fn stop_turn(&self) {
        self.with(Driver::stop_turn);
    }

    /// Stops the subagent or background item `task`.
    pub(crate) fn stop_task(&self, task: &str) {
        self.with(|driver| driver.stop_task(task));
    }

    /// Switches the session's model, where the path can while it runs.
    pub(crate) fn set_model(&self, model: &str) {
        if let Some(Err(why)) = self.with(|driver| driver.set_model(model)) {
            crate::common::diag::report("switching the model", why);
        }
    }

    /// Answers the permission request `request` with the input the user edited and the rules to keep.
    pub(crate) fn answer_permission_as(
        &self,
        request: &str,
        answer: term_claude::PermissionAnswer,
    ) -> Result<(), Explained> {
        let answered = self.with(|driver| driver.answer_permission_as(request, answer));
        after_answer(answered)
    }

    /// Answers the permission request `request` with the option at `option` in its list, or dismisses it.
    pub(crate) fn answer_option(&self, request: &str, option: Option<usize>) -> Result<(), Explained> {
        let answered = self.with(|driver| driver.answer_option(request, option));
        after_answer(answered)
    }

    /// Answers the questions of `request`.
    pub(crate) fn answer_questions(&self, request: &str, answers: Vec<Answer>) -> Result<(), Explained> {
        let answered = self.with(|driver| driver.answer_questions(request, answers));
        after_answer(answered)
    }

    /// Answers the plan `request`.
    pub(crate) fn answer_plan(&self, request: &str, reply: PlanReply) -> Result<(), Explained> {
        let answered = self.with(|driver| driver.answer_plan(request, reply));
        after_answer(answered)
    }

    /// Ends the session.
    pub(crate) fn leave(&self) {
        self.sessions.end(self.session);
    }
}

fn after_answer(answered: Option<Result<(), AnswerError>>) -> Result<(), Explained> {
    match answered {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(Explained(error.to_string())),
        None => Err(gone()),
    }
}

#[cfg(feature = "selftest")]
impl Driven {
    /// Plays recorded `lines` into the recording the session plays; what they changed, with the thread then, which
    /// the tab takes in itself, so its feed is emptied.
    pub(crate) fn feed(&self, lines: &str) -> (Vec<term_thread::Change>, Option<std::sync::Arc<Thread>>) {
        self.with(|driver| {
            if !driver.is_replay() {
                return (Vec::new(), Some(std::sync::Arc::new(driver.thread().clone())));
            }
            let changes = driver.feed(lines);
            let thread = driver.take(self.subscriber).map(|batch| batch.thread);
            (changes, thread)
        })
        .unwrap_or_default()
    }

    /// Answers the plan `request` of the recording the session plays; what changed, which the tab takes in itself.
    pub(crate) fn replay_plan(&self, request: &str, reply: PlanReply) -> Vec<term_thread::Change> {
        self.with(|driver| {
            let changes = driver.replay_plan(request, reply);
            driver.take(self.subscriber);
            changes
        })
        .unwrap_or_default()
    }
}

/// A new subscriber to `session`, with the path it takes, unless it is gone or has ended.
fn subscribe(sessions: &dyn Sessions, session: SessionId) -> Option<(SubscriberId, PathKind)> {
    let mut subscriber = None;
    sessions.with_driver(session, &mut |driver| {
        if !driver.is_ended() {
            subscriber = Some((driver.subscribe(), driver.kind()));
        }
    });
    subscriber
}

/// Why a call did not reach the session: it has ended.
pub(crate) fn gone() -> Explained {
    Explained::of(&term_claude::SessionError::Closed)
}
