//! Releases an idle Claude CLI while retaining its conversation and composer in the tab.

use std::cell::Cell;
use std::time::{Duration, Instant};

use crate::common::util::{main_after, main_soon};
use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use term_process::{ProcessId, Supervisor};
use term_thread::{Change, Lane};

use crate::Agent;
use crate::tab::AgentTab;

const CHECK_AFTER: Duration = Duration::from_secs(30);
const PARK_AFTER: Duration = Duration::from_secs(180);
const WAKE_CHECK: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct Parking {
    pub(super) hidden_since: Cell<Option<Instant>>,
    pub(super) process: Cell<Option<ProcessId>>,
    /// Whether a restored tab waits to start its CLI until it comes to the front or gets a prompt.
    pub(super) deferred: Cell<bool>,
    watching: Cell<bool>,
    waking: Cell<bool>,
}

impl AgentTab {
    pub(super) fn watch_for_parking(&self) {
        if self.parking.watching.replace(true) {
            return;
        }
        main_after(
            CHECK_AFTER,
            self.parking_callback(|tab| {
                tab.parking.watching.set(false);
                tab.consider_parking();
            }),
        );
    }

    fn parking_callback(&self, then: impl FnOnce(&AgentTab) + Send + 'static) -> impl FnOnce() + Send + 'static {
        let tab = MainThreadBound::new(self.me.borrow().clone(), self.mtm);
        move || {
            if let Some(tab) = MainThreadMarker::new().and_then(|mtm| tab.get(mtm).upgrade()) {
                then(&tab);
            }
        }
    }

    fn consider_parking(&self) {
        if self
            .state
            .borrow()
            .source()
            .session()
            .is_none_or(|session| session.is_ended())
        {
            self.parking.hidden_since.take();
            return;
        }
        self.watch_for_parking();
        let Some(session) = self.parkable_session() else {
            self.parking.hidden_since.take();
            return;
        };
        let since = self.parking.hidden_since.get().unwrap_or_else(Instant::now);
        self.parking.hidden_since.set(Some(since));
        if since.elapsed() < PARK_AFTER {
            return;
        }
        let process = session.process_id();
        let done = self.parking_callback(move |tab| tab.park_claude(process, since));
        let _ = term_base::lane::WORK.ask(
            move || session.has_process_work(),
            move |busy| {
                if busy == Some(false) {
                    main_soon(done);
                }
            },
        );
    }

    fn parkable_session(&self) -> Option<term_claude::Session> {
        if self.is_shown() || self.dialog.borrow().is_some() {
            return None;
        }
        let state = self.state.borrow();
        if state.running() || !state.queue().is_empty() || state.remote() == Some(true) {
            return None;
        }
        state.source().tab_session()?.parkable_claude()
    }

    fn park_claude(&self, process: ProcessId, since: Instant) {
        if self.parking.hidden_since.get() != Some(since)
            || !self
                .parkable_session()
                .is_some_and(|session| session.process_id() == process)
        {
            return;
        }
        self.persist();
        self.parking.process.set(Some(process));
        let source = self.state.borrow_mut().end();
        source.end();
        self.refresh();
    }

    /// Opens a restored Claude session as a parked one: its conversation read from the transcript, in a remote
    /// workspace from the machine there once its connection is open, its CLI started only once the tab comes to the
    /// front or gets a prompt. Other agents start at once.
    pub(crate) fn start_parked(&self) {
        if !matches!(self.agent, Agent::Claude) {
            self.start();
            return;
        }
        self.parking.deferred.set(true);
        self.refresh();
        let Some((transcript, session)) = self
            .state
            .borrow()
            .resume()
            .map(|r| (r.transcript.clone(), r.session.clone()))
        else {
            return;
        };
        if transcript.is_none() && !crate::remote_transcript::is_remote(&self.cwd) {
            return;
        }
        let tab = MainThreadBound::new(self.me.borrow().clone(), self.mtm);
        let thread = self.snapshot();
        let cwd = self.cwd.clone();
        let spawned = crate::common::util::spawn("claude-parked", move || {
            let rebuilt = match transcript {
                Some(path) => term_claude::history::rebuild(thread, &path).ok(),
                None => crate::remote_transcript::rebuild(
                    thread,
                    &cwd,
                    &session,
                    crate::remote_transcript::WAIT_FOR_CONNECTION,
                ),
            };
            let Some(rebuilt) = rebuilt else {
                return;
            };
            crate::common::util::give_back_memory_soon();
            main_soon(move || {
                // A start that came first and found no conversation, as one whose connection was not open yet,
                // takes it as well, as long as no session runs on it.
                if let Some(tab) = MainThreadMarker::new().and_then(|mtm| tab.get(mtm).upgrade())
                    && (tab.parking.deferred.get() || !tab.state.borrow().source().is_live())
                    && tab.state.borrow().thread().entries(&Lane::Main).is_empty()
                {
                    tab.absorb(rebuilt, &[Change::Cleared]);
                }
            });
        });
        if spawned.is_err() {
            self.start();
        }
    }

    /// Starts the CLI of a restored tab that waited for it, once the tab is in front and, in a remote workspace, its
    /// connection is open; the connection wakes it again when it opens.
    pub(crate) fn wake_deferred(&self) {
        if self.parking.deferred.get() && self.is_shown() && !term_remote::registry::waits_for_connection(&self.cwd) {
            self.start();
        }
    }

    /// Keeps the queued prompt until the old process has exited, then uses the usual Claude resume path; a restored
    /// tab that waited starts right away.
    pub(super) fn resume_parked(&self) {
        if self.state.borrow().queue().is_empty() {
            return;
        }
        let Some(process) = self.parking.process.get() else {
            if self.parking.deferred.get() && !term_remote::registry::waits_for_connection(&self.cwd) {
                self.start();
            }
            return;
        };
        if Supervisor::global().wait(process, Duration::ZERO).is_some() {
            self.start();
        } else if !self.parking.waking.replace(true) {
            main_after(
                WAKE_CHECK,
                self.parking_callback(|tab| {
                    tab.parking.waking.set(false);
                    tab.resume_parked();
                }),
            );
        }
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
