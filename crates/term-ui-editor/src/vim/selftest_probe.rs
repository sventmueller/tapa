//! The self-test's hooks of `vim.rs`.

use super::*;

/// The mode label of `view`'s status bar and whether the system caret is hidden for the block, for the self-test.
pub(crate) fn shown(view: &EditorTextView) -> (String, bool) {
    let label = view
        .ivars()
        .vim
        .borrow()
        .label
        .as_ref()
        .map(|l| l.stringValue().to_string())
        .unwrap_or_default();
    let hidden = view.insertionPointColor().alphaComponent() == 0.0;
    (label, hidden)
}

/// The UTF-16 range of the search match `view` marks, for the self-test.
pub(crate) fn marked(view: &EditorTextView) -> Option<std::ops::Range<usize>> {
    view.ivars().vim.borrow().marked.clone()
}

/// How many places Vim and the editor type at in `view`, for the self-test.
pub(crate) fn carets(view: &EditorTextView) -> usize {
    let vim = view.ivars().vim.borrow().vim.as_ref().map_or(1, Vim::carets);
    vim + super::super::multi::extra(view).len()
}

/// Whether the keyboard layout in use types ASCII, which decides how Vim reads a character outside Insert mode.
pub(crate) fn types_ascii() -> bool {
    keyboard::types_ascii()
}
