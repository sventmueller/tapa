//! The sessions that run, by the id the app keeps in its context (E97, E143); what asks before quitting, the
//! activity view, the closing of a window and `working_in` read them besides the open AI tabs (C58).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use term_process::Supervisor;

use crate::driver::Driver;
use crate::id::{SessionId, SubscriberId};
use crate::launch::{Launch, LaunchError};
use crate::rule::RequestRule;

/// The app's hop onto the main thread (E142): asked from any thread, it has `Registry::flush` run for `session` on
/// the main thread soon after, and once is enough until that flush ran.
pub trait Wake: Send + Sync {
    /// Asks for a flush of `session` on the main thread.
    fn wake(&self, session: SessionId);
}

impl<F: Fn(SessionId) + Send + Sync> Wake for F {
    fn wake(&self, session: SessionId) {
        self(session);
    }
}

/// The drivers of the running sessions.
pub struct Registry {
    supervisor: Supervisor,
    wake: Arc<dyn Wake>,
    next: u64,
    drivers: BTreeMap<SessionId, Driver>,
}

impl Registry {
    /// An empty registry whose sessions start their processes through `supervisor` and whose flushes `wake` brings
    /// onto the main thread.
    pub fn new(supervisor: Supervisor, wake: Arc<dyn Wake>) -> Registry {
        Registry {
            supervisor,
            wake,
            next: 0,
            drivers: BTreeMap::new(),
        }
    }

    /// Starts the session `launch` describes, answering its requests by `rule`. Its owners, such as the agent tab,
    /// subscribe to it before the main thread turns; it lives while one of them follows it.
    pub fn start(&mut self, launch: Launch, rule: Box<dyn RequestRule>) -> Result<SessionId, LaunchError> {
        self.next += 1;
        let id = SessionId(self.next);
        let driver = Driver::start(id, launch, rule, &self.supervisor, self.wake.clone())?;
        self.drivers.insert(id, driver);
        Ok(id)
    }

    /// Takes in what `session` reported, on the main thread; the subscribers to tell that a batch waits. A session
    /// nobody follows any more is stopped and let go.
    pub fn flush(&mut self, session: SessionId) -> Vec<SubscriberId> {
        let Some(driver) = self.drivers.get_mut(&session) else {
            return Vec::new();
        };
        let woken = driver.flush();
        if driver.subscribers() == 0 {
            self.remove(session);
        }
        woken
    }

    /// The driver of `session`.
    pub fn get(&self, session: SessionId) -> Option<&Driver> {
        self.drivers.get(&session)
    }

    /// The driver of `session`, to send, stop or answer.
    pub fn get_mut(&mut self, session: SessionId) -> Option<&mut Driver> {
        self.drivers.get_mut(&session)
    }

    /// Stops `session` and forgets it.
    pub fn remove(&mut self, session: SessionId) -> Option<Driver> {
        let mut driver = self.drivers.remove(&session)?;
        driver.stop();
        Some(driver)
    }

    /// Every session, oldest first.
    pub fn ids(&self) -> Vec<SessionId> {
        self.drivers.keys().copied().collect()
    }

    /// The sessions that work: a turn, a prompt that waits, a subagent or background item.
    pub fn working(&self) -> Vec<SessionId> {
        (self.drivers.iter())
            .filter(|(_, d)| d.is_working())
            .map(|(id, _)| *id)
            .collect()
    }

    /// The sessions whose agent works in `folder` or below it.
    pub fn working_in(&self, folder: &Path) -> Vec<SessionId> {
        (self.drivers.iter())
            .filter(|(_, d)| !d.is_ended() && d.folder().starts_with(folder))
            .map(|(id, _)| *id)
            .collect()
    }

    /// How many sessions there are.
    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }

    /// Stops every session, as the app quits.
    pub fn stop_all(&mut self) {
        for driver in self.drivers.values_mut() {
            driver.stop();
        }
    }
}
