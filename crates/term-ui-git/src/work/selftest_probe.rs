//! The self-test's hooks of `work.rs`.

use std::sync::atomic::{AtomicBool, Ordering};

static ISOLATED: AtomicBool = AtomicBool::new(false);

/// Keeps the user's own git configuration, such as signing or hooks paths, out of the git the panels start from now
/// on; the self-test calls it before the first workspace opens.
pub fn isolate_git_config() {
    ISOLATED.store(true, Ordering::Relaxed);
}

pub(crate) fn for_selftest(mut environment: Vec<(String, String)>) -> Vec<(String, String)> {
    if ISOLATED.load(Ordering::Relaxed) {
        environment.push(("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()));
        environment.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
    }
    environment
}
