//! The registry the agent tabs run their sessions in: one per app, its processes under the app's supervisor, its
//! flushes on the main thread, after which the tabs on the flushed session take in their feeds.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use term_agents::{Driver, Launch, LaunchError, Registry, SessionId, UserRule};
use term_process::Supervisor;

use crate::common::util::main_soon;

/// The app's registry as the agent tabs reach it.
pub(crate) struct Local {
    registry: RefCell<Registry>,
}

impl Local {
    /// An empty registry whose sessions start under the app's supervisor.
    pub(crate) fn new() -> Rc<Local> {
        let wake = |session: SessionId| main_soon(move || flush(session));
        Rc::new(Local {
            registry: RefCell::new(Registry::new(Supervisor::global().clone(), Arc::new(wake))),
        })
    }

    /// Stops every session, as the app quits.
    pub(crate) fn stop_all(&self) {
        self.registry.borrow_mut().stop_all();
    }
}

/// Takes in what `session` reported and hands it to the tabs on it.
fn flush(session: SessionId) {
    let state = crate::host::state();
    let Some(local) = state.registry() else { return };
    let fed = local.registry.borrow_mut().flush(session);
    drop(fed);
    crate::tab_driven::session_fed(&state, session);
}

impl crate::Sessions for Local {
    fn with_driver(&self, session: SessionId, f: &mut dyn FnMut(&mut Driver)) -> bool {
        let mut registry = self.registry.borrow_mut();
        match registry.get_mut(session) {
            Some(driver) => {
                f(driver);
                true
            }
            None => false,
        }
    }

    fn start(&self, launch: Launch) -> Result<SessionId, LaunchError> {
        self.registry.borrow_mut().start(launch, Box::new(UserRule))
    }

    fn end(&self, session: SessionId) {
        let removed = self.registry.borrow_mut().remove(session);
        drop(removed);
    }
}
