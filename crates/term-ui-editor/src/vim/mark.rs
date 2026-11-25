//! The match of Vim's last search, marked in the find colour until `:noh`, the next search or a change of the text.
//! Only the background is painted, which the colours of the syntax never touch.

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;

use super::EditorTextView;
use crate::util::constant;

/// Marks the match from the first place to the second of `view`'s text, each a line and a column in characters, or
/// none.
pub(super) fn show(view: &EditorTextView, found: Option<((usize, usize), (usize, usize))>) {
    clear(view);
    let Some((start, end)) = found else { return };
    let marked = super::utf16_of(view, start)..super::utf16_of(view, end);
    let (Some(layout), Some(content)) = (view.textLayoutManager(), view.textContentStorage()) else {
        return;
    };
    let Some(range) = crate::text_view::text_range(&content, &marked) else {
        return;
    };
    let marker: Retained<AnyObject> = objc2_app_kit::NSColor::findHighlightColor()
        .colorWithAlphaComponent(0.55)
        .into();
    // SAFETY: The value of the background colour attribute is a colour.
    unsafe {
        layout.addRenderingAttribute_value_forTextRange(
            constant!(objc2_app_kit::NSBackgroundColorAttributeName),
            Some(&marker),
            &range,
        );
    }
    view.ivars().vim.borrow_mut().marked = Some(marked);
}

/// Takes the mark away.
pub(super) fn clear(view: &EditorTextView) {
    let Some(marked) = view.ivars().vim.borrow_mut().marked.take() else {
        return;
    };
    let (Some(layout), Some(content)) = (view.textLayoutManager(), view.textContentStorage()) else {
        return;
    };
    if let Some(range) = crate::text_view::text_range(&content, &marked) {
        layout.removeRenderingAttribute_forTextRange(constant!(objc2_app_kit::NSBackgroundColorAttributeName), &range);
    }
}
