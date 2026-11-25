//! Where git runs: one app-wide `Git` once the login shell's environment is known, a lane for local work and threads
//! for work that may take minutes.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use term_base::LockIgnoringPoison;
use term_base::lane::Lane;
use term_git::Git;
use term_process::Supervisor;
use term_ui::kit::main_soon;

/// Local git work, one job at a time, so that a status read after a change sees it.
static LOCAL: Lane = Lane::with_start("term-git", 1, 64, term_base::priority::utility);

static GIT: OnceLock<Git> = OnceLock::new();

/// How long the first job waits for the login shell's environment before it gives up for this time.
const ENVIRONMENT_WAIT: Duration = Duration::from_secs(30);

/// The app's git; waits for the login shell's environment, so only for worker threads. `None` while it is not known.
pub fn git() -> Option<Git> {
    if let Some(git) = GIT.get() {
        return Some(git.clone());
    }
    let environment = term_ui::workspace_host::environment(ENVIRONMENT_WAIT);
    if environment.is_empty() {
        return None;
    }
    let environment = for_selftest(environment);
    Some(GIT.get_or_init(|| Git::new(Supervisor::global(), environment)).clone())
}

#[cfg(not(feature = "selftest"))]
fn for_selftest(environment: Vec<(String, String)>) -> Vec<(String, String)> {
    environment
}

/// Runs `work` on the git lane and hands its result to `done` on the main thread, `None` if it panicked; false when
/// the lane is full and nothing runs.
pub(crate) fn local<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(Option<T>) + Send + 'static,
) -> bool {
    LOCAL.ask(work, move |result| main_soon(move || done(result))).is_ok()
}

/// Runs `work` on a thread of its own, for hooks and the network, which may take minutes, and hands its result to
/// `done` on the main thread; `None` if it panicked or no thread could start.
pub(crate) fn long<T: Send + 'static, D: FnOnce(Option<T>) + Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: D,
) {
    let slot = Arc::new(Mutex::new(Some(done)));
    let theirs = slot.clone();
    let spawned = term_base::thread::detach("term-git-long", move || {
        term_base::priority::set_qos(term_base::priority::Qos::Utility);
        let result = term_base::lane::contained(work);
        if let Some(done) = theirs.lock_ignoring_poison().take() {
            main_soon(move || done(result));
        }
    });
    if let Err(unstarted) = spawned {
        drop(unstarted.work);
        term_base::diag::report("starting a git thread", unstarted.error);
        if let Some(done) = slot.lock_ignoring_poison().take() {
            main_soon(move || done(None));
        }
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;
