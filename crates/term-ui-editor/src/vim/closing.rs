//! `:q`, `:q!`, `:wq` and `:x`: they close the document of the editor they were typed in and no other, once the key
//! that asked is handled, and `:wq` and `:x` only after the file is written.

use objc2::DefinedClass;
use term_ui::workspace_host::Tabs;

use super::EditorTextView;

/// How the editor's document closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Closing {
    /// Asking about unsaved changes, as ⌘W does.
    Ask,
    /// Giving unsaved changes up, with their copy against a crash.
    Discard,
    /// Once the file is written; with `false` only when there are unsaved changes, else as it is.
    Save(bool),
}

/// Closes `view`'s document as `how` says once the key that asked is handled, which runs inside the editor's own
/// handling of the key.
pub(super) fn soon(view: &EditorTextView, how: Closing) {
    view.ivars().vim.borrow_mut().closing = Some(how);
    view.later(objc2::sel!(termVimClose), 0.0);
}

/// The close `soon` asked for.
pub(crate) fn due(view: &EditorTextView) {
    let Some(how) = view.ivars().vim.borrow_mut().closing.take() else {
        return;
    };
    match how {
        Closing::Ask => close(view, true),
        Closing::Discard => close(view, false),
        Closing::Save(false) if !view.document().is_dirty() => close(view, true),
        Closing::Save(_) => {
            written(view);
        }
    }
}

/// Writes `view`'s document, then closes it once the file is written: a write that failed shows
/// its error as ⌘S does and keeps the document open, as does an untitled one whose file the user did not pick.
fn written(view: &EditorTextView) {
    crate::save_then(
        view,
        false,
        Box::new(|view, written| {
            if written && !view.document().is_dirty() {
                close(view, true);
            }
        }),
    );
}

/// Closes `view`'s document, asking about unsaved changes with `ask`, else giving them up.
fn close(view: &EditorTextView, ask: bool) {
    let (Some(controller), Some(tab)) = (view.controller(), view.ivars().tab.get()) else {
        return;
    };
    if ask {
        controller.close_tab(tab);
    } else {
        crate::closing::give_up(view);
        controller.close_tab(tab);
    }
}
