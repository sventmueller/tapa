//! The AppKit shorthands of `term_ui::kit` and the main-thread hops of `term_base`, with what only the editor needs
//! beside them: one edit of a text storage, one undo step and a reset that runs when a guard drops.

use objc2_app_kit::{NSTextStorage, NSTextView};
use objc2_foundation::NSUndoManager;

pub(crate) use term_base::main_thread::{after_frame, main_on_run_loop};
pub(crate) use term_render::constant;
pub(crate) use term_ui::kit::{
    FlagGuard, Observation, activate, auto_layout, fixed_size, guarded, item, label, main_after, main_soon, ns,
    observe, pin_edges, pin_inset, pin_sides, separator, set_action, set_label, set_target, submenu, symbol, tagged,
};

/// One edit of a text storage, which it processes once the guard drops, also when a panic unwinds past it.
#[must_use = "the edit ends as soon as the guard drops"]
struct EditingGuard<'a>(&'a NSTextStorage);

impl Drop for EditingGuard<'_> {
    fn drop(&mut self) {
        self.0.endEditing();
    }
}

/// Runs `f` as one edit of `storage`.
pub(crate) fn with_editing<R>(storage: &NSTextStorage, f: impl FnOnce() -> R) -> R {
    storage.beginEditing();
    let _editing = EditingGuard(storage);
    f()
}

/// An undo group, which ends once the guard drops, also when a panic unwinds past it.
#[must_use = "the group ends as soon as the guard drops"]
struct UndoGroupGuard<'a> {
    undo: &'a NSUndoManager,
    by_event: bool,
}

impl<'a> UndoGroupGuard<'a> {
    /// Opens a group inside the one open, or else a top-level group of its own, during which the manager does not
    /// open one per event.
    fn begin(undo: &'a NSUndoManager) -> Self {
        let by_event = undo.groupsByEvent() && undo.groupingLevel() == 0;
        if by_event {
            undo.setGroupsByEvent(false);
        }
        undo.beginUndoGrouping();
        Self { undo, by_event }
    }
}

impl Drop for UndoGroupGuard<'_> {
    fn drop(&mut self) {
        self.undo.endUndoGrouping();
        if self.by_event {
            self.undo.setGroupsByEvent(true);
        }
    }
}

/// Runs `edit`, whose changes of `view` become one undoable step of their own, also right after typing in the same
/// turn of the run loop; inside a group a caller opened, they join it.
pub(crate) fn undo_step(view: &NSTextView, edit: impl FnOnce()) {
    view.breakUndoCoalescing();
    let undo = view.undoManager();
    if let Some(undo) = &undo
        && undo.groupsByEvent()
        && undo.groupingLevel() == 1
    {
        // AppKit closes the turn's group only at its end and until then refuses registrations without an open group,
        // so a fresh one replaces it, which the end of the turn or the next undo closes.
        undo.endUndoGrouping();
        undo.beginUndoGrouping();
    }
    let group = undo.as_deref().map(UndoGroupGuard::begin);
    edit();
    drop(group);
    view.breakUndoCoalescing();
}

/// Runs a closure once the guard drops, also when a panic unwinds past it: the reset of state that no other guard
/// covers, such as a field behind a `RefCell`.
#[must_use = "the closure runs as soon as the guard drops"]
pub(crate) struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnDrop<F> {
    /// The guard that runs `f` when it drops.
    pub(crate) fn new(f: F) -> Self {
        Self(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use objc2_foundation::NSString;

    use super::*;

    fn panics(f: impl FnOnce()) {
        assert!(catch_unwind(AssertUnwindSafe(f)).is_err());
    }

    #[test]
    fn a_panic_still_runs_the_reset() {
        let reset = Cell::new(false);
        panics(|| {
            let _reset = OnDrop::new(|| reset.set(true));
            panic!("in between");
        });
        assert!(reset.get());
    }

    #[test]
    fn a_panic_ends_the_edit_of_a_storage() {
        let storage = NSTextStorage::new();
        panics(|| {
            with_editing(&storage, || {
                storage.replaceCharactersInRange_withString(
                    objc2_foundation::NSRange::new(0, 0),
                    &NSString::from_str("a"),
                );
                assert!(!storage.editedMask().is_empty());
                panic!("in between");
            });
        });
        assert!(storage.editedMask().is_empty(), "the storage processed the edit");
    }
}
