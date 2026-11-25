//! The self-test's hooks of `indent.rs`.

use super::*;

/// The status bar's indentation title and charset warning of `view`, for the self-test.
pub(crate) fn shown(view: &EditorTextView) -> (String, Option<String>) {
    let state = view.ivars().spacing.borrow();
    let title = state.button.as_ref().map(|b| b.title().to_string()).unwrap_or_default();
    let warning = state
        .warning
        .as_ref()
        .filter(|w| !w.isHidden())
        .map(|w| w.stringValue().to_string());
    (title, warning)
}
