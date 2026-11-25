//! Whether the user is dragging a size right now: the window's edge, or a divider of a split view, which AppKit does
//! not report as a live resize. A terminal's grid reflows its history at every new width, so it does that at most
//! every 100 ms while a drag lasts.

use std::time::Duration;

use objc2_app_kit::{NSEvent, NSView};

/// How often the end of a drag is looked for.
const POLL: Duration = Duration::from_millis(50);

/// Whether a size is being dragged around `view`: its window resizes live, or a mouse button is held while its frame
/// changes, which is what a split view's divider drag looks like.
pub fn dragging(view: &NSView) -> bool {
    view.inLiveResize() || NSEvent::pressedMouseButtons() & 1 != 0
}

/// Runs `f` once no mouse button is held any longer, which ends a divider drag as well as a window resize; at once if
/// none is. `f` finds its view again by its id, as the view may be gone by then.
pub fn after_drag(f: impl FnOnce() + Send + 'static) {
    if NSEvent::pressedMouseButtons() & 1 == 0 {
        term_base::main_soon(f);
        return;
    }
    term_base::main_after(POLL, move || after_drag(f));
}
