//! What the self-test sets for the OpenCode route, kept out of `opencode.rs`'s production code.

use std::path::Path;

use term_base::LockIgnoringPoison;

/// The server every OpenCode tab starts in the self-test, without its folder; process-wide, since launches are
/// found on other threads. Unset, the OpenCode choice takes `opencode acp`, so no OpenCode on the Mac ever starts.
static TEST_OPENCODE: std::sync::Mutex<Option<term_opencode::Launch>> = std::sync::Mutex::new(None);

/// Starts every OpenCode tab's server with `launch`, as the self-test runs its stand-in.
pub fn set_test_opencode(launch: Option<term_opencode::Launch>) {
    *TEST_OPENCODE.lock_ignoring_poison() = launch;
}

pub(super) fn test_launch(cwd: &Path) -> Option<term_opencode::Launch> {
    TEST_OPENCODE.lock_ignoring_poison().clone().map(|mut launch| {
        launch.cwd = cwd.to_path_buf();
        launch
    })
}
