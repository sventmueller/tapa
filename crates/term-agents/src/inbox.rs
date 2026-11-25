//! What reaches a driver from the threads of its session and its workers, held until the app's hop to the main
//! thread empties it: control events in their order, invalidations coalesced as the AI tab's inbox does.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use term_base::explain::Explain;
use term_thread::Change;

use crate::id::SessionId;
use crate::registry::Wake;

/// Above this many distinct invalidations a view redraws from the snapshot instead (C71).
pub(crate) const MAX_CHANGES: usize = 4096;

/// Invalidations, each once; past `MAX_CHANGES` or after a `Cleared`, only `Cleared`.
#[derive(Debug, Default)]
pub(crate) struct Coalesced {
    changes: HashSet<Change>,
    cleared: bool,
}

impl Coalesced {
    /// Invalidations that start with everything to redraw.
    pub(crate) fn cleared() -> Coalesced {
        Coalesced {
            changes: HashSet::new(),
            cleared: true,
        }
    }

    pub(crate) fn add(&mut self, changes: impl IntoIterator<Item = Change>) {
        for change in changes {
            if self.cleared {
                return;
            }
            let full = self.changes.len() >= MAX_CHANGES && !self.changes.contains(&change);
            if change == Change::Cleared || full {
                self.changes.clear();
                self.cleared = true;
                return;
            }
            self.changes.insert(change);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.cleared && self.changes.is_empty()
    }

    /// What is held, emptied.
    pub(crate) fn take(&mut self) -> Vec<Change> {
        if std::mem::take(&mut self.cleared) {
            self.changes.clear();
            return vec![Change::Cleared];
        }
        self.changes.drain().collect()
    }
}

/// One control event of a session or a worker.
#[derive(Debug)]
pub(crate) enum Inbound {
    /// The path learned what the agent offers anew.
    Capabilities,
    /// The path reports a failure, in its own words.
    Failed(String),
    /// The process ended.
    Ended {
        /// The end of its error output.
        stderr: String,
    },
    /// The event stream broke off.
    Gap,
    /// The event stream is open again.
    Resumed,
    /// Reconciling with the server after a gap ended.
    Resynced(Result<(), String>),
    /// Claude Code reported a failure, typed; `Failed` with its words came just before.
    ClaudeFailed(term_claude::SessionError),
    /// Remote Control changed: Claude's connection.
    RemoteControl,
    /// Codex reported a failure, typed; `Failed` with its words came just before.
    CodexFailed(term_codex::SessionError),
    /// OpenCode reported a failure, typed; `Failed` with its words came just before.
    OpenCodeFailed(term_opencode::SessionError),
    /// An ACP agent reported a failure, typed; `Failed` with its words came just before.
    AcpFailed(term_acp::SessionError),
    /// Anything the driver does not act on, such as an MCP server's state; it only wakes the driver.
    Other,
}

#[derive(Debug, Default)]
pub(crate) struct Pending {
    pub(crate) inbound: Vec<Inbound>,
    pub(crate) changes: Coalesced,
}

/// The inbox of one driver, shared with the threads that report to it.
pub(crate) struct Inbox {
    session: SessionId,
    wake: Arc<dyn Wake>,
    pending: Mutex<Pending>,
    scheduled: AtomicBool,
}

impl Inbox {
    pub(crate) fn new(session: SessionId, wake: Arc<dyn Wake>) -> Arc<Inbox> {
        Arc::new(Inbox {
            session,
            wake,
            pending: Mutex::new(Pending::default()),
            scheduled: AtomicBool::new(false),
        })
    }

    fn with(&self, f: impl FnOnce(&mut Pending)) {
        f(&mut self.pending.lock().unwrap_or_else(PoisonError::into_inner));
        self.poke();
    }

    /// Asks the app to empty the inbox on the main thread, unless that is asked already.
    pub(crate) fn poke(&self) {
        if !self.scheduled.swap(true, Ordering::SeqCst) {
            self.wake.wake(self.session);
        }
    }

    pub(crate) fn push(&self, inbound: Inbound) {
        self.with(|pending| pending.inbound.push(inbound));
    }

    pub(crate) fn changed(&self, changes: Vec<Change>) {
        self.with(|pending| pending.changes.add(changes));
    }

    /// Everything that came, emptied; what comes from now on asks for another hop.
    pub(crate) fn take(&self) -> Pending {
        self.scheduled.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn claude(self: &Arc<Self>) -> impl Fn(term_claude::Event) + Send + Sync + 'static {
        let inbox = self.clone();
        move |event| match event {
            term_claude::Event::Changed(changes) => inbox.changed(changes),
            term_claude::Event::Capabilities => inbox.push(Inbound::Capabilities),
            term_claude::Event::Failed(error) => {
                inbox.push(Inbound::Failed(error.explain()));
                inbox.push(Inbound::ClaudeFailed(error));
            }
            term_claude::Event::Ended { stderr } => inbox.push(Inbound::Ended { stderr }),
            term_claude::Event::RemoteControl => inbox.push(Inbound::RemoteControl),
        }
    }

    pub(crate) fn codex(self: &Arc<Self>) -> impl Fn(term_codex::Event) + Send + Sync + 'static {
        let inbox = self.clone();
        move |event| match event {
            term_codex::Event::Changed(changes) => inbox.changed(changes),
            term_codex::Event::Capabilities => inbox.push(Inbound::Capabilities),
            term_codex::Event::Failed(error) => {
                inbox.push(Inbound::Failed(error.to_string()));
                inbox.push(Inbound::CodexFailed(error));
            }
            term_codex::Event::Ended { stderr } => inbox.push(Inbound::Ended { stderr }),
            _ => inbox.push(Inbound::Other),
        }
    }

    pub(crate) fn opencode(self: &Arc<Self>) -> impl Fn(term_opencode::Event) + Send + Sync + 'static {
        let inbox = self.clone();
        move |event| match event {
            term_opencode::Event::Changed(changes) => inbox.changed(changes),
            term_opencode::Event::Capabilities => inbox.push(Inbound::Capabilities),
            term_opencode::Event::Failed(error) => {
                inbox.push(Inbound::Failed(error.to_string()));
                inbox.push(Inbound::OpenCodeFailed(error));
            }
            term_opencode::Event::Ended { stderr } => inbox.push(Inbound::Ended { stderr }),
            term_opencode::Event::StreamGap => inbox.push(Inbound::Gap),
            term_opencode::Event::StreamResumed => inbox.push(Inbound::Resumed),
            term_opencode::Event::McpChanged(_) => inbox.push(Inbound::Other),
        }
    }

    pub(crate) fn acp(self: &Arc<Self>) -> impl Fn(term_acp::Event) + Send + Sync + 'static {
        let inbox = self.clone();
        move |event| match event {
            term_acp::Event::Changed(changes) => inbox.changed(changes),
            term_acp::Event::Capabilities => inbox.push(Inbound::Capabilities),
            term_acp::Event::Failed(error) => {
                inbox.push(Inbound::Failed(error.explain()));
                inbox.push(Inbound::AcpFailed(error));
            }
            term_acp::Event::Ended { stderr } => inbox.push(Inbound::Ended { stderr }),
        }
    }

    #[cfg(any(test, feature = "selftest"))]
    pub(crate) fn script(self: &Arc<Self>) -> impl Fn(crate::script::Event) + Send + Sync + 'static {
        let inbox = self.clone();
        move |event| match event {
            crate::script::Event::Changed(changes) => inbox.changed(changes),
            crate::script::Event::Ended { stderr } => inbox.push(Inbound::Ended { stderr }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_invalidations_stay_bounded_and_past_the_limit_become_one_redraw() {
        let mut held = Coalesced::default();
        for _ in 0..10_000 {
            held.add([Change::Usage]);
        }
        assert_eq!(held.take(), [Change::Usage]);
        assert!(held.is_empty());
        held.add((0..=MAX_CHANGES).map(|n| Change::Subagent(n.to_string())));
        assert_eq!(held.take(), [Change::Cleared]);
        held.add([Change::Todos, Change::Cleared, Change::Usage]);
        assert_eq!(held.take(), [Change::Cleared]);
        assert!(!Coalesced::cleared().is_empty());
    }

    #[test]
    fn the_app_is_asked_once_until_the_inbox_is_emptied() {
        let woken = Arc::new(Mutex::new(Vec::new()));
        let heard = woken.clone();
        let inbox = Inbox::new(
            SessionId(3),
            Arc::new(move |id: SessionId| heard.lock().unwrap().push(id)),
        );
        inbox.changed(vec![Change::Usage]);
        inbox.push(Inbound::Capabilities);
        inbox.push(Inbound::Gap);
        assert_eq!(*woken.lock().unwrap(), [SessionId(3)]);
        let pending = inbox.take();
        assert!(matches!(
            pending.inbound.as_slice(),
            [Inbound::Capabilities, Inbound::Gap]
        ));
        inbox.push(Inbound::Other);
        assert_eq!(woken.lock().unwrap().len(), 2);
    }
}
