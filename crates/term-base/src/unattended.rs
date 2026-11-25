//! Whether anyone answers the app's questions; nobody does in the self-test, so questions take the answer that lets
//! the step go on.

use std::sync::atomic::{AtomicBool, Ordering};

static UNATTENDED: AtomicBool = AtomicBool::new(false);

/// From now on nobody answers.
pub fn set() {
    UNATTENDED.store(true, Ordering::Relaxed);
}

/// Whether nobody answers.
pub fn is() -> bool {
    UNATTENDED.load(Ordering::Relaxed)
}

/// From now on somebody answers again, as a self-test step that answers the questions itself does.
pub fn unset() {
    UNATTENDED.store(false, Ordering::Relaxed);
}
