//! What the self-test reads from and does with the parts of `source.rs`, kept out of its production code.

use std::rc::Rc;
use std::time::{Duration, SystemTime};

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;
use crate::driven::{Driven, Sessions};

/// A replay into `thread` in the registry `sessions`, as the self-test plays it; its first batch is the thread the
/// tab already shows, so it is not taken in again.
pub(crate) fn replay(sessions: &Rc<dyn Sessions>, thread: Thread) -> Option<Source> {
    let cwd = thread.cwd().to_path_buf();
    let replay = term_claude::replay::Replay::new(thread, SystemTime::now(), Duration::from_millis(40));
    let launch = term_agents::Launch::new(
        term_agents::AgentPath::Replay(Box::new(replay)),
        cwd,
        Vec::new(),
        term_agents::UserChoices::default(),
    );
    match Driven::start(sessions, launch) {
        Ok((driven, _first)) => Some(Source::Session(driven)),
        Err(error) => {
            crate::common::diag::report("playing a recording", error);
            None
        }
    }
}

/// The program the self-test starts as `claude`; process-wide, since the lookup runs on a worker thread.
pub(super) static TEST_PROGRAM: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Starts every Claude tab with `program` instead of the `claude` on the Mac, as the self-test runs a stand-in.
pub fn set_test_program(program: PathBuf) {
    *TEST_PROGRAM.lock_ignoring_poison() = Some(program);
}

impl Source {
    /// Plays recorded lines into the replay; what changed, with the thread then.
    pub(crate) fn feed(&mut self, lines: &str) -> (Vec<term_thread::Change>, Option<std::sync::Arc<Thread>>) {
        match self.tab_session() {
            Some(driven) => driven.feed(lines),
            None => (Vec::new(), self.snapshot().map(std::sync::Arc::new)),
        }
    }
}
