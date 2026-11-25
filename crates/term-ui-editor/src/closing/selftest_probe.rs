//! Saving every editor for the self-test, which checks the files right after.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use super::{Again, Answer, dirty_count, save_each, unsaved};

/// How long a self-test waits for its saves.
const SAVE_WAIT: Duration = Duration::from_secs(10);

/// Saves every editor with unsaved changes, those of gone files aside, and waits for the writes; whether all of them
/// were saved.
pub(crate) fn save_all() -> bool {
    let over = Rc::new(Cell::new(false));
    let heard = Rc::clone(&over);
    let then: Again = Rc::new(move |_| heard.set(true));
    if save_each(unsaved(), then) == Answer::Later {
        crate::saving::run_until(|| over.get(), SAVE_WAIT);
    }
    dirty_count() == 0
}

/// `save_below` for the self-test, waiting for the writes; whether none is left unsaved below `root`.
pub(crate) fn save_below_now(root: &Path) -> bool {
    let saved = Rc::new(Cell::new(None));
    let heard = Rc::clone(&saved);
    super::save_below(root, Box::new(move |all| heard.set(Some(all))));
    crate::saving::run_until(|| saved.get().is_some(), SAVE_WAIT);
    saved.get() == Some(true)
}
