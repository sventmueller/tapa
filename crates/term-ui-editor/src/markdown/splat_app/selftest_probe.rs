//! Saving for Splat in the self-test, which checks the file right after.

use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

/// `saved`, waiting for the writes; whether none is left unsaved.
pub(crate) fn saved_now(path: &Path) -> bool {
    let all = Rc::new(Cell::new(None));
    let heard = Rc::clone(&all);
    super::saved(path, Box::new(move |done| heard.set(Some(done))));
    crate::run_until(|| all.get().is_some(), Duration::from_secs(10));
    all.get() == Some(true)
}
