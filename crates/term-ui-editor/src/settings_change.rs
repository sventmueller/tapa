//! What the editors do when the user's settings changed outside their own switches, and the zoom of an editor's text.

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::{EditorTextView, open_editors, opening, state, vim, whitespace};

/// The smallest size the settings offer for the code.
pub const FONT_MIN: f64 = 8.0;
/// The largest size the settings offer for the code.
pub const FONT_MAX: f64 = 36.0;

/// Shows the code font at its new size, reads whether to show whitespace and take Vim's keys again, and follows them
/// in every editor.
pub(super) fn apply() {
    let size = crate::host::settings().font_size;
    for view in open_editors() {
        opening::set_font_size(&view, size);
    }
    whitespace::forget();
    state().vim.forget();
    for view in open_editors() {
        vim::attach(&view);
    }
}

/// The open editor that `responder`, the view with the keyboard, belongs to: its text or a field of its find bar.
fn around(responder: &NSView) -> Option<Retained<EditorTextView>> {
    open_editors()
        .into_iter()
        .find(|editor| match editor.enclosingScrollView() {
            Some(scroll) => responder.isDescendantOf(&scroll),
            None => responder.isDescendantOf(editor),
        })
}

/// Whether `responder` is in an editor.
pub(super) fn holds(responder: &NSView) -> bool {
    around(responder).is_some()
}

/// Shows the text of the editor around `responder` `change` points larger, within the settings' range, or at the
/// settings' size again for a change of 0; the settings stay as they are.
pub(super) fn zoom(responder: &NSView, change: f64) {
    let Some(view) = around(responder) else { return };
    let size = if change == 0.0 {
        crate::host::settings().font_size
    } else {
        (view.ivars().font_size.get() + change).clamp(FONT_MIN, FONT_MAX)
    };
    opening::set_font_size(&view, size);
}
