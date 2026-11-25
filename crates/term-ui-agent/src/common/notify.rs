//! Whether the user would miss what happens in an agent tab, and the notification that tells them; the app posts it
//! through `Host::post_notice`.

use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use term_ui::workspace_host::Workspace;

static POSTED: AtomicUsize = AtomicUsize::new(0);

/// Whether the user would miss what happens in a tab that is `shown` now.
pub fn user_looks_elsewhere(shown: bool) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    !NSApplication::sharedApplication(mtm).isActive() || !shown
}

/// Tells the user that the tab `key` of `workspace` finished or waits, titled `title`.
pub fn post(workspace: &Workspace, key: &str, title: &str, body: &str) {
    POSTED.fetch_add(1, Ordering::Relaxed);
    crate::host::host().post_notice(workspace.id(), key, title, body);
}

/// How many notifications the agent tabs posted.
pub fn posted() -> usize {
    POSTED.load(Ordering::Relaxed)
}
