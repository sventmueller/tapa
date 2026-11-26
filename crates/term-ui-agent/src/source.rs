//! Where a tab's thread comes from: its own session in the registry, on any path, which a recording played for the
//! self-test can take the place of.

use std::path::PathBuf;
#[cfg(feature = "selftest")]
use term_base::LockIgnoringPoison;
use term_base::explain::{Explain, Explained};

use term_claude::{Capabilities, PermissionAnswer, PlanAnswer, Session, SessionError};
use term_process::tool;
use term_thread::{Answer, Thread, UserMessage};

use crate::driven::Driven;
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

/// A past session to continue.
#[derive(Debug, Clone)]
pub struct Resume {
    /// The CLI's id for it.
    pub session: String,
    /// The app's id for its thread, when the index knows it.
    pub thread: Option<term_thread::ThreadId>,
    /// The title it had.
    pub title: Option<String>,
    /// The transcript the CLI saved, whose conversation is shown before it continues.
    pub transcript: Option<PathBuf>,
    /// The register's id of the agent of a session the app keeps no thread of, such as a Codex thread from outside
    /// the app; Claude's when none.
    pub agent: Option<String>,
}

/// What keeps a tab from running a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Problem {
    /// The agent cannot skip the project's configuration and the workspace is not trusted (E16); the words for the
    /// user.
    Untrusted(String),
    /// No `claude` on the login shell's PATH.
    NotFound,
    /// The CLI has no usable sign-in.
    SignIn(String),
    /// The process did not start.
    Start(String),
    /// The agent's program is neither on the login shell's PATH outside the workspace nor installed by the app;
    /// the installation the app offers, if any.
    Missing(Option<Box<term_acp::Plan>>),
    /// The app installs the agent; how far it got.
    Installing(String),
    /// The ACP agent wants a sign-in before it opens a session.
    AcpSignIn,
    /// The ACP agent's program ended on its own after its session opened; the last line it wrote to stderr.
    Exited(Option<String>),
}

pub(crate) enum Source {
    /// Nothing runs yet, or any more.
    Idle,
    /// Looking for `claude` and the login shell's environment.
    Starting,
    /// The tab's own session, which the app's registry runs: Claude, Codex or OpenCode over their own protocols, an
    /// agent over ACP; in the self-test, a played recording.
    Session(Driven),
}

impl Source {
    /// The Claude session of the tab's own, for what only Claude Code does.
    pub(crate) fn session(&self) -> Option<Session> {
        self.tab_session().and_then(Driven::claude)
    }

    /// The tab's own session in the registry, or the recording that plays instead.
    pub(crate) fn tab_session(&self) -> Option<&Driven> {
        match self {
            Source::Session(driven) => Some(driven),
            _ => None,
        }
    }

    /// The thread as of the driver's last flush, when there is one.
    pub(crate) fn snapshot(&self) -> Option<Thread> {
        self.tab_session().and_then(Driven::snapshot)
    }

    pub(crate) fn capabilities(&self) -> Option<Capabilities> {
        match self {
            Source::Session(driven) => driven.capabilities(),
            _ => None,
        }
    }

    pub(crate) fn is_live(&self) -> bool {
        match self {
            Source::Session(driven) => driven.is_live(),
            Source::Idle | Source::Starting => false,
        }
    }

    /// The ACP session, when the tab runs one.
    pub(crate) fn acp(&self) -> Option<term_acp::Session> {
        self.tab_session().and_then(Driven::acp)
    }

    pub(crate) fn acp_capabilities(&self) -> Option<term_acp::Capabilities> {
        if let Some(session) = self.codex() {
            return Some(crate::codex::capabilities(&session.capabilities()));
        }
        if let Some(session) = self.opencode() {
            return Some(crate::opencode::capabilities(&session.capabilities()));
        }
        self.acp().map(|session| session.capabilities())
    }

    /// The OpenCode session, when the tab runs one over its server.
    pub(crate) fn opencode(&self) -> Option<term_opencode::Session> {
        self.tab_session().and_then(Driven::opencode)
    }

    /// The Codex session, when the tab runs one over the app-server.
    pub(crate) fn codex(&self) -> Option<term_codex::Session> {
        self.tab_session().and_then(Driven::codex)
    }

    pub(crate) fn authenticate(&self, method: &str) -> Result<(), term_acp::SessionError> {
        self.acp()
            .map_or(Err(term_acp::SessionError::Closed), |s| s.authenticate(method))
    }

    pub(crate) fn set_option(&self, option: &str, value: &str) -> Result<(), term_acp::SessionError> {
        if let Some(session) = self.opencode() {
            return session
                .set_option(option, value)
                .map_err(|e| term_acp::SessionError::Agent(crate::opencode::explained(&e)));
        }
        if let Some(session) = self.codex() {
            return session
                .set_option(option, value)
                .map_err(|e| term_acp::SessionError::Agent(crate::codex::explained(&e)));
        }
        self.acp()
            .map_or(Err(term_acp::SessionError::Closed), |s| s.set_option(option, value))
    }

    pub(crate) fn set_acp_mode(&self, mode: &str) -> Result<(), term_acp::SessionError> {
        if let Some(session) = self.opencode() {
            return session
                .set_mode(mode)
                .map_err(|e| term_acp::SessionError::Agent(crate::opencode::explained(&e)));
        }
        if let Some(session) = self.codex() {
            return session
                .set_mode(mode)
                .map_err(|e| term_acp::SessionError::Agent(crate::codex::explained(&e)));
        }
        self.acp()
            .map_or(Err(term_acp::SessionError::Closed), |s| s.set_mode(mode))
    }

    /// Answers the permission request `request` with the option at `option` in its list, or dismisses it.
    pub(crate) fn answer_option(&self, request: &str, option: Option<usize>) -> Result<(), Explained> {
        match self.tab_session() {
            Some(driven) => driven.answer_option(request, option),
            None => Err(Explained::of(&SessionError::Closed)),
        }
    }

    /// Sends `message` as a prompt, into the turn that runs for Claude; the CLI's id for it, which an ACP agent does
    /// not give.
    pub(crate) fn send(&self, message: UserMessage) -> Result<Option<String>, Explained> {
        match self {
            Source::Session(driven) => driven.steer(message),
            _ => Err(Explained::of(&SessionError::Closed)),
        }
    }

    pub(crate) fn interrupt(&self) {
        if let Some(driven) = self.tab_session() {
            driven.stop_turn();
        }
    }

    /// Answers the permission request `request`, with an edited input and rules to keep where Claude takes them.
    pub(crate) fn answer(&self, request: &str, answer: PermissionAnswer) -> Result<(), Explained> {
        match self.tab_session().filter(|driven| !driven.is_replay()) {
            Some(driven) => driven.answer_permission_as(request, answer),
            None => Err(Explained::of(&SessionError::Closed)),
        }
    }

    /// Answers the questions of request `request`, whichever agent asked them; an MCP server's form of Codex takes
    /// the answers as its fields.
    pub(crate) fn answer_questions(&self, request: &str, answers: Vec<Answer>) -> Result<(), Explained> {
        match self.tab_session().filter(|driven| !driven.is_replay()) {
            Some(driven) => driven.answer_questions(request, answers),
            None => Err(Explained::of(&SessionError::Closed)),
        }
    }

    /// Answers a plan; what a recording played for the self-test changed, which a session reports as an event.
    pub(crate) fn answer_plan(&self, request: &str, answer: PlanAnswer) -> Result<Vec<term_thread::Change>, Explained> {
        let reply = match answer {
            PlanAnswer::Approve => term_agents::PlanReply::Approve,
            PlanAnswer::Revise(feedback) => term_agents::PlanReply::Revise(feedback),
            PlanAnswer::Reject => term_agents::PlanReply::Reject,
        };
        #[cfg(feature = "selftest")]
        if let Some(driven) = self.tab_session().filter(|driven| driven.is_replay()) {
            return Ok(driven.replay_plan(request, reply));
        }
        match self.tab_session() {
            Some(driven) => driven.answer_plan(request, reply).map(|()| Vec::new()),
            None => Err(Explained::of(&SessionError::Closed)),
        }
    }

    pub(crate) fn stop_task(&self, task: &str) {
        if let Some(driven) = self.tab_session() {
            driven.stop_task(task);
        }
    }

    pub(crate) fn background(&self, call: &str) {
        if let Some(session) = self.session() {
            session.background(Some(call)).then(|_| ());
        }
    }

    /// Switches the session's permission mode; `refused` gets the CLI's reason when it says no.
    pub(crate) fn set_mode(&self, mode: &str, refused: impl FnOnce(String) + Send + 'static) {
        if let Some(session) = self.session() {
            session
                .set_permission_mode(term_claude::PermissionMode::from(mode))
                .then(move |answer| {
                    if let Err(error) = answer {
                        refused(error.explain());
                    }
                });
        }
    }

    pub(crate) fn set_model(&self, model: &str) {
        if let Some(driven) = self.tab_session() {
            driven.set_model(model);
        }
    }

    pub(crate) fn set_effort(&self, level: &str) {
        if let Some(session) = self.session() {
            session.set_effort(term_claude::Effort::from(level)).then(|_| ());
        }
    }

    /// Ends the process: input closed, then the supervisor's signals, which the registry sends.
    pub(crate) fn end(&self) {
        if let Some(driven) = self.tab_session() {
            driven.leave();
        }
    }
}

/// The `claude` executable on the login shell's PATH, else in the places its installers use.
pub(crate) fn find_claude(env: &[(String, String)]) -> Option<PathBuf> {
    #[cfg(feature = "selftest")]
    if let Some(program) = TEST_PROGRAM.lock_ignoring_poison().clone() {
        // A remote workspace's stand-in starts the stand-in the self-test puts on the machine there.
        let remote =
            tool::which("claude", tool::var(env, "PATH")).filter(|found| term_remote::registry::is_stand_in(found));
        return Some(remote.unwrap_or(program));
    }
    tool::which("claude", tool::var(env, "PATH")).or_else(|| {
        let home = term_base::home();
        [
            home.join(".local/bin"),
            home.join(".claude/local"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
        .into_iter()
        .map(|dir| dir.join("claude"))
        .find(|candidate| tool::is_executable(candidate))
    })
}

/// What a failure Claude Code reports means for the tab.
pub(crate) fn problem_of(error: &SessionError) -> Option<Problem> {
    match error {
        SessionError::SignInRequired(why) => Some(Problem::SignIn(why.clone())),
        SessionError::NotFound(_) => Some(Problem::NotFound),
        SessionError::Start(why) => Some(Problem::Start(why.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_is_found_on_the_path_the_login_shell_gives() {
        let dir = term_base::testing::scratch("find-claude");
        let exe = dir.join("claude");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = vec![("PATH".to_string(), format!("/nonexistent:{}", dir.display()))];
        assert_eq!(find_claude(&env), Some(exe.clone()));
        let overridden = vec![
            ("PATH".to_string(), "/nonexistent".to_string()),
            ("PATH".to_string(), dir.display().to_string()),
        ];
        assert_eq!(
            find_claude(&overridden),
            Some(exe),
            "the last PATH of the list is the one a child sees"
        );
    }
}
