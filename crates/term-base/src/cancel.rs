//! A request to stop a long task, such as an installation, a search, a git command or a provider's answer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::sync::LockIgnoringPoison;

/// A request to stop a long task, shared between the asker and the task; clones share it. A task that polls asks
/// [`Cancel::is_cancelled`]; a task that blocks, such as on a connection, sets a hook with [`Cancel::on_cancel`].
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicBool, Ordering};
///
/// use term_base::Cancel;
///
/// let cancel = Cancel::new();
/// let closed = Arc::new(AtomicBool::new(false));
/// let hook = closed.clone();
/// cancel.on_cancel(move || hook.store(true, Ordering::SeqCst));
/// cancel.clone().cancel();
/// assert!(cancel.is_cancelled() && closed.load(Ordering::SeqCst));
/// ```
#[derive(Clone, Default)]
pub struct Cancel(Arc<State>);

#[derive(Default)]
struct State {
    cancelled: AtomicBool,
    hooks: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl Cancel {
    /// A request that has not been made yet.
    pub fn new() -> Cancel {
        Cancel::default()
    }

    /// Asks the task to stop as soon as it can, and runs the hooks it set, once, on the caller's thread; a hook only
    /// closes a connection or sends a note, so this does not block.
    pub fn cancel(&self) {
        if self.0.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        let hooks = std::mem::take(&mut *self.0.hooks.lock_ignoring_poison());
        for hook in hooks {
            hook();
        }
    }

    /// Whether stopping was asked for.
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Runs `hook` when stopping is asked for, or now if it already was; how a task that blocks is stopped.
    pub fn on_cancel(&self, hook: impl FnOnce() + Send + 'static) {
        let mut hooks = self.0.hooks.lock_ignoring_poison();
        if self.is_cancelled() {
            drop(hooks);
            hook();
        } else {
            hooks.push(Box::new(hook));
        }
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Cancel").field(&self.is_cancelled()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_clone_hears_the_request() {
        let cancel = Cancel::new();
        let task = cancel.clone();
        assert!(!task.is_cancelled());
        cancel.cancel();
        assert!(task.is_cancelled());
    }

    #[test]
    fn hooks_run_once_when_cancelled_and_at_once_when_set_afterwards() {
        let cancel = Cancel::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let counted = runs.clone();
        cancel.on_cancel(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        cancel.cancel();
        cancel.clone().cancel();
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let late = runs.clone();
        cancel.on_cancel(move || {
            late.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 11);
    }

    #[test]
    fn a_hook_may_cancel_again_or_set_another_hook() {
        let cancel = Cancel::new();
        let again = cancel.clone();
        let ran = Arc::new(AtomicBool::new(false));
        let inner = ran.clone();
        cancel.on_cancel(move || {
            again.cancel();
            again.on_cancel(move || inner.store(true, Ordering::SeqCst));
        });
        cancel.cancel();
        assert!(ran.load(Ordering::SeqCst));
    }
}
