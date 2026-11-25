//! How an agent tab takes in its own session in the registry: the session's feed, whose first batch is the
//! snapshot, and the signals that come with it.

use std::rc::Rc;

use term_agents::{PathKind, SessionId, Signal, TurnId};
use term_base::explain::Explained;
use term_thread::Change;

use super::driven::Driven;
use super::source::{self, Problem};
use super::tab::AgentTab;

/// Hands the agent tabs on `session` what its driver published for them, after the registry's flush of it.
pub(crate) fn session_fed(state: &crate::State, session: SessionId) {
    let tabs: Vec<Rc<AgentTab>> = (state.agent_tabs.tabs.borrow().iter())
        .filter(|tab| tab.fed_session() == Some(session))
        .cloned()
        .collect();
    for tab in tabs {
        tab.take_feed();
    }
}

impl AgentTab {
    /// The registry's session whose feed the tab takes in.
    fn fed_session(&self) -> Option<SessionId> {
        self.state.borrow().source().tab_session().map(Driven::session)
    }

    /// Takes in what the session's feed holds for the tab: the snapshot first, then the changes since.
    pub(crate) fn take_feed(&self) {
        let own = self.state.borrow().source().tab_session().cloned();
        if let Some(own) = own {
            self.take_own_feed(&own);
        }
    }

    /// Takes in what the tab's own session reported since the last time, as its events did when the tab ran the
    /// session itself.
    fn take_own_feed(&self, own: &Driven) {
        let Some(batch) = own.take() else {
            self.refresh();
            return;
        };
        if batch.changes.is_empty() && batch.signals.is_empty() {
            return;
        }
        let mut heard = Heard::default();
        for signal in batch.signals {
            self.hear(signal, &mut heard);
        }
        match own.kind() {
            PathKind::Claude | PathKind::Replay => self.take_claude(heard, batch.thread, &batch.changes),
            _ => self.take_other(heard, batch.thread, &batch.changes),
        }
    }

    /// Notes in `heard` what `signal` of the tab's own session means, and hands on at once what concerns the message
    /// the driver took.
    fn hear(&self, signal: Signal, heard: &mut Heard) {
        match signal {
            Signal::Capabilities => heard.learned = true,
            Signal::RemoteControl => heard.remote = true,
            Signal::ClaudeFailed(error) => heard.problem = source::problem_of(&error),
            Signal::CodexFailed(error) => heard.problem = Some(Problem::Start(crate::codex::explained(&error))),
            Signal::OpenCodeFailed(error) => heard.problem = Some(Problem::Start(crate::opencode::explained(&error))),
            Signal::AcpFailed(error) => heard.problem = self.acp_problem(&error),
            Signal::Delivered { turn, sent, .. } => self.went_out(turn, sent.map_err(Explained)),
            Signal::TurnEnded(end) => self.went_out(end.turn, Err(super::driven::gone())),
            Signal::Ended { stderr } => {
                let handed = self.state.borrow().handed_turn();
                if let Some(turn) = handed {
                    self.went_out(turn, Err(super::driven::gone()));
                }
                heard.ended = Some(stderr);
            }
            _ => {}
        }
    }

    /// Takes in what the tab's own Claude session, or the recording that plays instead, reported.
    fn take_claude(&self, heard: Heard, thread: std::sync::Arc<term_thread::Thread>, changes: &[Change]) {
        if heard.learned {
            self.state.borrow_mut().learn();
        }
        if heard.problem.is_some() {
            self.set_problem(heard.problem);
        }
        self.absorb(thread, changes);
        super::dialogs::requests_changed(self);
        if heard.learned {
            self.refresh();
            super::dialogs::session_ready(self);
        } else if heard.remote {
            self.refresh_remote();
        }
    }

    /// Takes in what the tab's own session on another path than Claude reported: Codex, OpenCode or ACP.
    fn take_other(&self, heard: Heard, thread: std::sync::Arc<term_thread::Thread>, changes: &[Change]) {
        if heard.learned {
            self.state.borrow_mut().learn();
            let codex = self.state.borrow().source().codex();
            if let Some(session) = codex {
                crate::codex::remember_threads(&session, &self.cwd);
            }
            if self.agent.is_acp() {
                self.acp_apply_choice();
                self.send_queued();
            }
        }
        if heard.problem.is_some() {
            self.set_problem(heard.problem);
        }
        self.absorb(thread, changes);
        super::dialogs::requests_changed(self);
        if heard.learned {
            self.refresh();
            super::dialogs::session_ready(self);
        } else if heard.remote {
            self.refresh_remote();
        }
        if let Some(stderr) = heard.ended {
            self.acp_ended(&stderr);
        }
    }

    /// The message the driver took for `turn` went out, or did not: the agent refused it, or the turn or the session
    /// ended before it went out. A message that went out before is left alone. A message of an agent that gives
    /// prompts no id keeps the app's own.
    fn went_out(&self, turn: TurnId, sent: Result<Option<String>, Explained>) {
        let Some(message) = self.state.borrow_mut().take_handed(turn) else {
            return;
        };
        let app_id = message
            .message_id
            .clone()
            .filter(|_| super::actions::app_ids(&self.agent));
        if sent.is_err() {
            self.not_sent(&message);
        }
        self.state.borrow_mut().delivering();
        self.delivered(&message.text, sent.map(|id| id.or(app_id)));
    }
}

/// What the signals of one batch of the tab's own session said, beyond what is handed on at once.
#[derive(Default)]
struct Heard {
    /// What the agent offers changed.
    learned: bool,
    /// Remote Control changed.
    remote: bool,
    /// What keeps the session from working.
    problem: Option<Problem>,
    /// The end of the agent's error output, once its process ended.
    ended: Option<String>,
}
