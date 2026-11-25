//! What the editor took from Dango's language server crate without the servers (E16): the serial that tells editors
//! apart off the main thread, TextKit 2 ranges and rectangles of UTF-16 offsets, and edits made back to front.

use std::cell::Cell;
use std::cmp::Reverse;
use std::ops::Range;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextContentStorage, NSTextInputClient, NSTextRange, NSTextView};
use objc2_foundation::{NSRange, NSRect};
use term_document::Change;

use crate::util::{ns, undo_step};

/// The number that tells an editor apart from every other one while the app runs, which callbacks from other threads
/// carry instead of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EditorSerial(u64);

thread_local! {
    static NEXT: Cell<u64> = const { Cell::new(1) };
}

/// A serial no editor had before.
pub(crate) fn new_serial() -> EditorSerial {
    NEXT.with(|next| {
        let id = next.get();
        next.set(id + 1);
        EditorSerial(id)
    })
}

/// The TextKit 2 range of the UTF-16 offsets `range`.
pub(crate) fn text_range(content: &NSTextContentStorage, range: &Range<usize>) -> Option<Retained<NSTextRange>> {
    use objc2_app_kit::NSTextElementProvider;
    let start = content.documentRange().location();
    let from = content.locationFromLocation_withOffset(&start, isize::try_from(range.start).ok()?)?;
    let to = content.locationFromLocation_withOffset(&start, isize::try_from(range.end).ok()?)?;
    NSTextRange::initWithLocation_endLocation(NSTextRange::alloc(), &from, Some(&to))
}

/// The rectangle of `range`'s first line in `view`'s coordinates; TextKit 2 answers this for input methods.
pub(crate) fn rect_of(view: &NSTextView, range: Range<usize>) -> Option<NSRect> {
    let window = view.window()?;
    // SAFETY: A range within the text; no actual range is asked for.
    let screen = unsafe {
        view.firstRectForCharacterRange_actualRange(
            NSRange::new(range.start, range.end - range.start),
            std::ptr::null_mut(),
        )
    };
    let in_window = window.convertRectFromScreen(screen);
    Some(view.convertRect_fromView(in_window, None))
}

/// Moves `range` of UTF-16 offsets along with `change`; whether the change left it whole, outside the range.
pub(crate) fn follow(range: &mut Range<usize>, change: &Change) -> bool {
    let old = &change.utf16;
    if range.end <= old.start {
        return true;
    }
    if range.start >= old.end {
        range.start = range.start - (old.end - old.start) + change.text_utf16;
        range.end = range.end - (old.end - old.start) + change.text_utf16;
        return true;
    }
    let new_end = old.start + change.text_utf16;
    range.start = range.start.min(old.start);
    range.end = if range.end <= old.end {
        new_end
    } else {
        range.end - (old.end - old.start) + change.text_utf16
    };
    false
}

/// `edits` in the order that keeps the offsets of those still to come: from the last to the first, and of edits at
/// the same offset the later one first.
fn back_to_front(edits: Vec<(Range<usize>, String)>) -> Vec<(Range<usize>, String)> {
    let mut numbered: Vec<(usize, (Range<usize>, String))> = edits.into_iter().enumerate().collect();
    numbered.sort_by_key(|(index, (range, _))| Reverse((range.start, *index)));
    numbered.into_iter().map(|(_, edit)| edit).collect()
}

/// Makes `edits`, UTF-16 ranges of the current text with their new text, in `view` as one undo step; whether the view
/// took all of them.
pub(crate) fn apply_text_edits(view: &crate::EditorTextView, edits: Vec<(Range<usize>, String)>) -> bool {
    let Some(storage) = view.storage() else { return false };
    let mut took = true;
    undo_step(view, || {
        for (range, text) in back_to_front(edits) {
            let range = NSRange::new(range.start, range.end - range.start);
            let text = ns(&text);
            if view.shouldChangeTextInRange_replacementString(range, Some(&text)) {
                storage.replaceCharactersInRange_withString(range, &text);
                view.didChangeText();
            } else {
                took = false;
            }
        }
    });
    took
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_go_from_the_last_to_the_first() {
        let edits = vec![(0..1, "a".into()), (5..6, "b".into()), (5..5, "c".into())];
        let order: Vec<String> = back_to_front(edits).into_iter().map(|(_, text)| text).collect();
        assert_eq!(order, ["c", "b", "a"]);
    }

    #[test]
    fn serials_are_never_handed_out_twice() {
        assert_ne!(new_serial(), new_serial());
    }
}
