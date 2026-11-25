//! The AppKit shorthands of `term_ui::kit` and the main-thread hops of `term_base`, with a thread for one piece of
//! work and one undo step beside them.

use objc2_app_kit::NSTextView;
use objc2_foundation::NSUndoManager;

pub(crate) use term_render::constant;
pub(crate) use term_ui::kit::{guarded, main_after, main_soon, ns, set_action, symbol};

/// Runs `f` on a new thread called `name`; the error says why the thread did not start.
pub(crate) fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), String> {
    term_base::thread::detach(name, f).map_err(|unstarted| unstarted.error.to_string())
}

/// An undo group, which ends once the guard drops, also when a panic unwinds past it.
#[must_use = "the group ends as soon as the guard drops"]
struct UndoGroupGuard<'a> {
    undo: &'a NSUndoManager,
    by_event: bool,
}

impl<'a> UndoGroupGuard<'a> {
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

/// Runs `edit`, whose changes of `view` become one undoable step of their own, also right after typing.
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
