//! The login shell's environment, which agents start with so that they find what the user's PATH holds: read once in
//! the background in the home folder, as nothing from a project folder may run (E16).

use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

type Shared = (Mutex<Option<Vec<(String, String)>>>, Condvar);

/// How long the login shell may take before the app's own environment stands in.
const CAPTURE_LIMIT: Duration = Duration::from_secs(10);

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| (Mutex::new(None), Condvar::new()))
}

/// Starts reading the environment; once.
pub(crate) fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    term_base::thread::detach_or_run("login environment", || {
        let env = if own_environment() {
            std::env::vars().collect()
        } else {
            let shell = term_base::shell_env::user_shell();
            term_base::shell_env::capture(&shell, &term_base::home(), CAPTURE_LIMIT).unwrap_or_else(|error| {
                term_base::diag::report("reading the login shell's environment", format!("{error:?}"));
                std::env::vars().collect()
            })
        };
        let (lock, ready) = shared();
        *lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(env);
        ready.notify_all();
    });
}

/// The environment, waiting up to `wait` for it.
pub(crate) fn environment(wait: Duration) -> Option<Vec<(String, String)>> {
    let (lock, ready) = shared();
    let guard = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (guard, _) = ready
        .wait_timeout_while(guard, wait, |env| env.is_none())
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clone()
}

/// Whether the app's own environment counts: in a self-test, whose runner puts the stand-ins on its PATH.
fn own_environment() -> bool {
    cfg!(feature = "selftest") && std::env::var_os("TAPA_SELFTEST").is_some()
}
