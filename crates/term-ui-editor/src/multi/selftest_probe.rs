//! The self-test's hooks of `multi.rs`.

use super::*;

/// Puts carets at UTF-16 offsets `at`, the last one the view's own, for the self-test.
pub(crate) fn set(view: &EditorTextView, mut at: Vec<Range<usize>>) {
    let Some(primary) = at.pop() else { return };
    view.ivars().carets.borrow_mut().extra = at;
    select(view, &primary);
    normalize(view);
}

/// Every caret, sorted, for the self-test.
pub(crate) fn carets(view: &EditorTextView) -> Vec<Range<usize>> {
    all(view).0
}
