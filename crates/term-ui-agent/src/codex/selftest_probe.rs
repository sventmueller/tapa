//! What the self-test sets for the Codex route, kept out of `codex.rs`'s production code.

use std::path::Path;

use term_base::LockIgnoringPoison;

/// The app-server every Codex tab starts in the self-test, without its folder; process-wide, since launches are
/// found on other threads. Unset, the Codex choice takes the ACP adapter, so no Codex on the Mac ever starts.
static TEST_CODEX: std::sync::Mutex<Option<term_codex::Launch>> = std::sync::Mutex::new(None);

/// Starts every Codex tab's app-server with `launch`, as the self-test runs its stand-in.
pub fn set_test_codex(launch: Option<term_codex::Launch>) {
    *TEST_CODEX.lock_ignoring_poison() = launch;
}

pub(super) fn test_launch(cwd: &Path) -> Option<term_codex::Launch> {
    TEST_CODEX.lock_ignoring_poison().clone().map(|mut launch| {
        launch.cwd = cwd.to_path_buf();
        launch
    })
}
