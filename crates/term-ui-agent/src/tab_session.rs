//! How an agent tab's session runs: starting it, the events that come from it, and what changes of its status set
//! off.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use splat_i18n::tf;
use term_agents::{AgentPath, Launch, LaunchError, UserChoices};
use term_thread::{Change, Status, Thread};
use term_ui::workspace_host::{Notices, Trust};

use super::driven::Driven;
use super::notify;
use super::source::{self, Problem, Source};
use super::tab::AgentTab;
use crate::AgentKey;
use crate::common::util::{main_after, main_soon};

impl AgentTab {
    /// Looks for the agent's program and the login shell's environment on a worker thread, then starts the session;
    /// in a workspace the user does not trust, without the project's configuration (E16).
    pub(crate) fn start(&self) {
        self.parking.deferred.set(false);
        if self.controller().is_none() {
            return;
        }
        self.parking.process.take();
        let before = self.state.borrow_mut().start();
        before.end();
        self.refresh();
        if self.agent.is_acp() {
            let thread = self.state.borrow().thread().as_ref().clone();
            self.start_acp(thread);
            return;
        }
        let transcript = self.state.borrow().resume().and_then(|r| r.transcript.clone());
        let key = self.key;
        let cwd = self.cwd.clone();
        let mut thread = self.snapshot();
        thread.apply_all(super::waiting::hidden(self.state.borrow().queue()));
        if let Err(why) = crate::common::util::spawn("claude-start", move || {
            let env = term_ui::workspace_host::environment_for(&cwd, Duration::from_secs(15));
            let exe = source::find_claude(&env);
            let thread = match transcript {
                Some(path) if thread.entries(&term_thread::Lane::Main).is_empty() => {
                    let rebuilt = term_claude::history::rebuild(thread.clone(), &path).unwrap_or(thread);
                    crate::common::util::give_back_memory_soon();
                    rebuilt
                }
                _ => thread,
            };
            main_soon(move || {
                if let Some(tab) = super::get(key) {
                    tab.launch(exe, env, thread);
                }
            });
        }) {
            self.start_failed(Problem::Start(why.0));
        }
    }

    /// Starts the tab's Claude session in the app's registry; a `claude` inside the workspace never runs (E16).
    pub(super) fn launch(&self, exe: Option<PathBuf>, env: Vec<(String, String)>, mut thread: Thread) {
        thread.apply_all(super::waiting::shown(self.state.borrow().queue()));
        let Some(exe) = exe.filter(|exe| !exe.starts_with(&self.cwd)) else {
            self.start_failed(Problem::NotFound);
            return;
        };
        let Some(sessions) = self.sessions.clone() else {
            self.start_failed(Problem::Start(String::new()));
            return;
        };
        let launch = self.claude_launch(exe, env, thread);
        match Driven::start(&sessions, launch) {
            Ok((driven, first)) => {
                self.run_session(driven);
                if let Some(first) = first {
                    self.absorb(first.thread, &[Change::Cleared]);
                }
                self.send_queued();
            }
            Err(error) => {
                let problem = match error {
                    LaunchError::NotFound(_) => Problem::NotFound,
                    LaunchError::Start(why) => Problem::Start(why),
                    LaunchError::Untrusted(why) => Problem::Untrusted(why),
                };
                self.start_failed(problem);
            }
        }
    }

    /// Runs `driven`, the tab's own session the registry started, on any path; one a start that overtook this one
    /// began ends.
    pub(super) fn run_session(&self, driven: Driven) {
        let claude = driven.kind() == term_agents::PathKind::Claude;
        let replaced = self.state.borrow_mut().started(Source::Session(driven));
        replaced.end();
        if claude {
            self.watch_for_parking();
        }
    }

    /// The launch of the tab's Claude session with `exe` in `env` on `thread`, as the user's picks, the sandbox
    /// switch, Remote Control and the workspace's trust set it.
    fn claude_launch(&self, exe: PathBuf, env: Vec<(String, String)>, thread: Thread) -> Launch {
        let state = self.state.borrow();
        let picks = state.picks();
        let resume = (thread.session().map(str::to_string)).or_else(|| state.resume().map(|r| r.session.clone()));
        let path = AgentPath::Claude {
            program: exe,
            remote_control: state.remote(),
            rewind: None,
        };
        let choices = UserChoices {
            mode: Some(picks.mode().to_string()),
            sandbox: state.sandbox(),
        };
        let mut launch = Launch::new(path, &self.cwd, env, choices);
        launch.model = picks.model().map(str::to_string);
        launch.effort = picks.effort().map(str::to_string);
        launch.resume = resume;
        launch.thread = Some(thread);
        launch.trusted = self.controller.is_trusted();
        launch
    }

    /// What an ACP session's failure means for the tab; failures of a running session's requests are only logged.
    pub(super) fn acp_problem(&self, error: &term_acp::SessionError) -> Option<Problem> {
        let ready = self.state.borrow().source().is_live();
        let problem = acp_problem_of(self.agent.name(), ready, error);
        if problem.is_none() {
            crate::common::diag::report("a request to the agent", error);
        }
        problem
    }

    pub(super) fn status_moved(&self, before: Status, now: Status) {
        let title = self.title();
        let elsewhere = notify::user_looks_elsewhere(self.is_shown());
        match now {
            Status::Idle if before != Status::Idle => {
                if let Some(controller) = self.controller() {
                    crate::host::host().turn_ended(controller.id(), &self.cwd);
                }
                if self.restart_after_turn() {
                    return;
                }
                self.send_queued();
            }
            Status::RequiresAction if elsewhere => {
                notify::post(
                    &self.controller,
                    &self.tab_key(),
                    &title,
                    &tf("{} wartet auf deine Antwort.", &[self.agent.name()]),
                );
            }
            _ => {}
        }
        self.refresh_tab_mark();
    }

    /// Tells the user who looks elsewhere that the session finished, once its turn, every subagent and its background
    /// work have ended and it stayed so for `FINISHED_SETTLE`.
    pub(super) fn work_moved(&self, busy_before: bool, busy_now: bool) {
        if busy_before == busy_now {
            return;
        }
        let epoch = self.quiet_epoch.get().wrapping_add(1);
        self.quiet_epoch.set(epoch);
        if busy_now {
            return;
        }
        let bound = dispatch2::MainThreadBound::new(self.me.borrow().clone(), self.mtm);
        main_after(FINISHED_SETTLE, move || {
            let Some(mtm) = objc2::MainThreadMarker::new() else {
                return;
            };
            let Some(tab) = bound.into_inner(mtm).upgrade() else {
                return;
            };
            let quiet =
                tab.state.borrow().status() == Status::Idle && !super::tab::delegates(tab.state.borrow().thread());
            if tab.quiet_epoch.get() != epoch || !quiet || !notify::user_looks_elsewhere(tab.is_shown()) {
                return;
            }
            tab.attention.set(true);
            notify::post(
                &tab.controller,
                &tab.tab_key(),
                &tab.title(),
                &tf("{} ist fertig.", &[tab.agent.name()]),
            );
            tab.refresh_tab_mark();
        });
    }

    /// Advances running times every second while anything runs.
    pub(super) fn start_ticking(&self) {
        if self.ticking.replace(true) {
            return;
        }
        tick(self.key);
    }
}

/// How long a session stays idle before it counts as finished: Claude starts the next turn by itself once a subagent,
/// a background command or a stop hook hands it something, which leaves the session idle for a moment in between.
pub(crate) const FINISHED_SETTLE: Duration = Duration::from_secs(3);

/// What the failure `error` of the ACP agent `name` means for its tab, with `ready` once the session runs; `None` for
/// a failed request of a running session.
fn acp_problem_of(name: &str, ready: bool, error: &term_acp::SessionError) -> Option<Problem> {
    match error {
        term_acp::SessionError::SignIn => Some(Problem::AcpSignIn),
        term_acp::SessionError::Version(v) => Some(Problem::Start(tf(
            "{} spricht die Protokollversion {}, diese App Version 1",
            &[name, &v.to_string()],
        ))),
        other if !ready => Some(Problem::Start(term_base::explain::Explain::explain(other))),
        _ => None,
    }
}

/// One second of running time for the tab `key`, again while anything runs.
fn tick(key: AgentKey) {
    let Some(tab) = super::get(key) else { return };
    let live = tab.timeline.tick(SystemTime::now());
    let running = tab.status() != Status::Idle;
    if running && let Some(session) = tab.session() {
        super::usage::fetch(&session);
    }
    if live || running {
        main_after(Duration::from_secs(1), move || tick(key));
    } else {
        tab.ticking.set(false);
    }
    if running && let Some(controller) = tab.controller() {
        controller.refresh_activity();
    }
}

#[cfg(test)]
mod tests {
    use term_acp::SessionError;
    use term_thread::{
        BackgroundItem, BackgroundKind, OutputTail, SubagentPatch, TaskStatus, Thread, ThreadId, Update,
    };

    use super::*;
    use crate::tab::delegates;

    #[test]
    fn a_running_subagent_workflow_or_background_command_keeps_the_session_working_but_housekeeping_does_not() {
        let mut thread = Thread::new(ThreadId::from("t"), "claude", "/work");
        let item = |kind, ambient| BackgroundItem {
            id: format!("{kind:?}"),
            kind,
            label: "work".into(),
            status: TaskStatus::Running,
            started: None,
            ended: None,
            tool_call: None,
            parent: None,
            output: OutputTail::default(),
            is_backgrounded: true,
            ambient,
        };
        for kind in [BackgroundKind::Shell, BackgroundKind::Monitor, BackgroundKind::Workflow] {
            thread.apply(Update::Background(vec![item(kind, false)]));
            assert!(delegates(&thread));
        }
        thread.apply(Update::Background(vec![item(BackgroundKind::Monitor, true)]));
        assert!(!delegates(&thread));
        thread.apply(Update::Background(Vec::new()));
        assert!(!delegates(&thread));
        let mut sub = SubagentPatch::new("toolu_1", SystemTime::UNIX_EPOCH);
        sub.status = Some(TaskStatus::Running);
        thread.apply(Update::Subagent(sub));
        assert!(delegates(&thread));
        let mut done = SubagentPatch::new("toolu_1", SystemTime::UNIX_EPOCH);
        done.status = Some(TaskStatus::Completed);
        thread.apply(Update::Subagent(done));
        assert!(!delegates(&thread));
    }

    #[test]
    fn a_sign_in_or_a_foreign_protocol_stops_the_tab_even_while_running() {
        assert_eq!(
            acp_problem_of("Gemini", true, &SessionError::SignIn),
            Some(Problem::AcpSignIn)
        );
        let Some(Problem::Start(why)) = acp_problem_of("Gemini", true, &SessionError::Version(2)) else {
            panic!("a version mismatch is a start problem");
        };
        assert!(why.contains("Gemini") && why.contains('2'), "{why}");
    }

    #[test]
    fn other_failures_stop_only_a_tab_that_is_still_starting() {
        let error = SessionError::Agent("boom".into());
        assert_eq!(
            acp_problem_of("Gemini", false, &error),
            Some(Problem::Start("boom".into()))
        );
        assert_eq!(acp_problem_of("Gemini", true, &error), None);
        assert_eq!(acp_problem_of("Gemini", true, &SessionError::Closed), None);
    }
}
