//! More than one caret: ⌥-click adds one, ⌥-drag selects a column, ⌃⇧↑/↓ add one above or below, ⌘D the next
//! occurrence of the selection. NSTextView keeps one selection and drops empty ranges from several, so the others
//! live here, and every edit or move runs once per caret, from the last to the first, as one undoable step; the
//! document model sees each edit as it is made.

use std::ops::Range;

use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, sel};
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSPasteboard, NSPasteboardTypeString,
    NSSelectionGranularity, NSTextInputClient, NSTextLayoutManagerSegmentOptions, NSTextLayoutManagerSegmentType,
};
use objc2_foundation::{NSNotFound, NSPoint, NSRange, NSRect, NSStringCompareOptions};
use term_document::Change;

use super::view::EditorTextView;
use crate::util::{OnDrop, constant, ns};

/// The carets and selections beyond NSTextView's own.
#[derive(Debug, Default)]
pub(crate) struct Carets {
    /// In UTF-16 code units, sorted, apart from the view's selection.
    extra: Vec<Range<usize>>,
    /// While an operation runs at every caret: where each caret ended so far, by its place in document order.
    done: Vec<(usize, Range<usize>)>,
    replaying: bool,
    /// Set while this module moves the view's selection, which then does not count as the user's.
    selecting: bool,
}

fn range_of(r: NSRange) -> Range<usize> {
    r.location..r.location + r.length
}

fn ns_range(r: &Range<usize>) -> NSRange {
    NSRange::new(r.start, r.end - r.start)
}

/// The selections beyond the view's own.
pub(crate) fn extra(view: &EditorTextView) -> Vec<Range<usize>> {
    view.ivars().carets.borrow().extra.clone()
}

/// Whether the view has more than one caret and is not already running an operation at each.
fn active(view: &EditorTextView) -> bool {
    let carets = view.ivars().carets.borrow();
    !carets.extra.is_empty() && !carets.replaying
}

/// Back to one caret.
pub(crate) fn clear(view: &EditorTextView) {
    let had = !view.ivars().carets.borrow().extra.is_empty();
    view.ivars().carets.borrow_mut().extra.clear();
    if had {
        super::overlay::redraw(view);
    }
}

/// Keeps the carets on their text while it changes.
pub(crate) fn shift(view: &EditorTextView, change: &Change) {
    let Ok(mut carets) = view.ivars().carets.try_borrow_mut() else {
        return;
    };
    if carets.replaying {
        return;
    }
    for range in &mut carets.extra {
        crate::text_view::follow(range, change);
    }
}

/// The view's selection changed: unless this module moved it, the user, or undo, went back to one caret.
pub(crate) fn selection_changed(view: &EditorTextView) {
    let ours = view
        .ivars()
        .carets
        .try_borrow()
        .is_ok_and(|c| c.replaying || c.selecting);
    if !ours {
        clear(view);
    }
    super::overlay::redraw(view);
}

fn select(view: &EditorTextView, range: &Range<usize>) {
    view.ivars().carets.borrow_mut().selecting = true;
    let _selecting = OnDrop::new(|| view.ivars().carets.borrow_mut().selecting = false);
    view.setSelectedRange(ns_range(range));
}

/// Every selection, the view's included, sorted and merged where they meet, with the place of the view's own.
fn all(view: &EditorTextView) -> (Vec<Range<usize>>, usize) {
    merge(range_of(view.selectedRange()), extra(view))
}

/// `primary` and `extra` sorted and merged where they overlap or repeat, with the place of the range holding `primary`.
fn merge(primary: Range<usize>, mut ranges: Vec<Range<usize>>) -> (Vec<Range<usize>>, usize) {
    ranges.push(primary.clone());
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = vec![];
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start < last.end || range == *last => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    let index = merged
        .iter()
        .position(|r| r.start <= primary.start && primary.end <= r.end)
        .unwrap_or(0);
    (merged, index)
}

/// Runs `op` at every caret, from the last to the first, as one undoable step; `op` gets the caret's place in
/// document order. The carets done move by how much the text grew, because the storage may report an operation's
/// edits as one change that spans carets.
fn at_every_caret(view: &EditorTextView, op: impl Fn(&EditorTextView, usize)) {
    let (ranges, primary) = all(view);
    {
        let mut carets = view.ivars().carets.borrow_mut();
        carets.replaying = true;
        carets.done.clear();
    }
    let replaying = OnDrop::new(|| view.ivars().carets.borrow_mut().replaying = false);
    super::undo_step(view, || {
        for (index, range) in ranges.iter().enumerate().rev() {
            let before = view.string().length();
            view.setSelectedRange(ns_range(range));
            op(view, index);
            let grown = isize::try_from(view.string().length()).unwrap_or(0) - isize::try_from(before).unwrap_or(0);
            let now = range_of(view.selectedRange());
            let mut carets = view.ivars().carets.borrow_mut();
            for (_, done) in &mut carets.done {
                *done = done.start.saturating_add_signed(grown)..done.end.saturating_add_signed(grown);
            }
            carets.done.push((index, now));
        }
    });
    drop(replaying);
    let (main, others) = {
        let mut carets = view.ivars().carets.borrow_mut();
        let mut done = std::mem::take(&mut carets.done);
        done.sort_by_key(|(index, _)| *index);
        let main = done.iter().find(|(i, _)| *i == primary).map(|(_, r)| r.clone());
        let others: Vec<Range<usize>> = done
            .into_iter()
            .filter(|(i, _)| *i != primary)
            .map(|(_, r)| r)
            .collect();
        (main, others)
    };
    view.ivars().carets.borrow_mut().extra = others;
    if let Some(main) = main {
        select(view, &main);
    }
    normalize(view);
}

/// Merges carets that ran into each other, as deleting between them does.
fn normalize(view: &EditorTextView) {
    let (merged, index) = all(view);
    let primary = merged.get(index).cloned();
    let others: Vec<Range<usize>> = merged
        .into_iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, r)| r)
        .collect();
    view.ivars().carets.borrow_mut().extra = others;
    if let Some(primary) = primary {
        select(view, &primary);
    }
    super::overlay::redraw(view);
}

fn insert_text(view: &EditorTextView, text: &AnyObject) {
    // SAFETY: The inserted object is the string the keyboard or the pasteboard gave, one of the types the method takes.
    unsafe { view.insertText_replacementRange(text, NSRange::new(NSNotFound.cast_unsigned(), 0)) };
}

/// Text typed while there are several carets goes to each; whether it was taken.
pub(crate) fn insert(view: &EditorTextView, text: &AnyObject) -> bool {
    if !active(view) {
        return false;
    }
    at_every_caret(view, |view, _| insert_text(view, text));
    true
}

/// A command of the keyboard while there are several carets: moves, deletions and insertions run at each, Esc
/// leaves one, anything else leaves one and runs as usual. Whether it was taken.
pub(crate) fn command(view: &EditorTextView, command: Sel) -> bool {
    if !active(view) {
        return false;
    }
    if command == sel!(cancelOperation:) {
        clear(view);
        return true;
    }
    let name = command.name().to_string_lossy();
    let everywhere = [
        "move",
        "delete",
        "insert",
        "transpose",
        "capitalize",
        "lowercase",
        "uppercase",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix));
    if !everywhere {
        clear(view);
        return false;
    }
    // SAFETY: The selector is one the keyboard sent this view, which it answers as it did a moment ago.
    at_every_caret(view, |view, _| unsafe { view.doCommandBySelector(command) });
    true
}

/// Pastes one line of the clipboard per caret when it holds as many lines, else all of it at each; whether it was
/// taken.
pub(crate) fn paste(view: &EditorTextView) -> bool {
    if !active(view) {
        return false;
    }
    let pasteboard = NSPasteboard::generalPasteboard();
    if let Some(text) = pasteboard.stringForType(constant!(NSPasteboardTypeString)) {
        paste_text(view, &text.to_string());
    }
    true
}

/// Inserts `text` at every caret, one line each when it has as many lines as there are carets.
pub(crate) fn paste_text(view: &EditorTextView, text: &str) {
    let count = all(view).0.len();
    let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(text).split('\n').collect();
    let pieces: Vec<String> = if lines.len() == count {
        lines.iter().map(|l| l.trim_end_matches('\r').to_string()).collect()
    } else {
        vec![text.to_string(); count]
    };
    at_every_caret(view, |view, index| {
        insert_text(view, &ns(pieces.get(index).map_or("", String::as_str)));
    });
}

/// Copies every selection, one line each; whether it was taken.
pub(crate) fn copy(view: &EditorTextView) -> bool {
    if !active(view) {
        return false;
    }
    if let Some(text) = copied(view) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&ns(&text), constant!(NSPasteboardTypeString));
    }
    true
}

/// The text of every selection that is not empty, one per line, in document order.
pub(crate) fn copied(view: &EditorTextView) -> Option<String> {
    let string = view.string();
    let parts: Vec<String> = all(view)
        .0
        .iter()
        .filter(|r| !r.is_empty())
        .map(|r| string.substringWithRange(ns_range(r)).to_string())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Copies and then deletes every selection; whether it was taken.
pub(crate) fn cut(view: &EditorTextView) -> bool {
    if !copy(view) {
        return false;
    }
    at_every_caret(view, |view, _| {
        if view.selectedRange().length > 0 {
            insert_text(view, &ns(""));
        }
    });
    true
}

/// Adds a caret at UTF-16 offset `at`; the view's selection becomes one of the others.
fn add_caret(view: &EditorTextView, at: Range<usize>) {
    let primary = range_of(view.selectedRange());
    if primary == at || extra(view).contains(&at) {
        return;
    }
    view.ivars().carets.borrow_mut().extra.push(primary);
    select(view, &at);
    normalize(view);
    view.scrollRangeToVisible(ns_range(&at));
}

/// Adds a caret on the line above the first caret, or below the last, at the same column or the line's end.
pub(crate) fn add_vertical(view: &EditorTextView, up: bool) {
    let (ranges, _) = all(view);
    let edge = if up { ranges.first() } else { ranges.last() };
    let Some(edge) = edge.cloned() else { return };
    let target = {
        let document = view.document();
        let text = document.text();
        let Ok(byte) = text.utf16_to_byte(edge.end.min(text.len_utf16())) else {
            return;
        };
        let Ok(point) = text.point(byte) else { return };
        let line = if up {
            match point.line.checked_sub(1) {
                Some(line) => line,
                None => return,
            }
        } else if point.line + 1 < text.line_count() {
            point.line + 1
        } else {
            return;
        };
        let column = u32::try_from(point.utf16).unwrap_or(u32::MAX);
        let line = u32::try_from(line).unwrap_or(u32::MAX);
        text.byte_to_utf16(text.lsp_offset(line, column, term_document::Unit::Utf16))
            .unwrap_or(0)
    };
    add_caret(view, target..target);
}

/// Selects the word at the cursor, or adds the next occurrence of the selection as a further selection.
pub(crate) fn add_next_occurrence(view: &EditorTextView) {
    let primary = range_of(view.selectedRange());
    if primary.is_empty() {
        let word =
            view.selectionRangeForProposedRange_granularity(ns_range(&primary), NSSelectionGranularity::SelectByWord);
        select(view, &range_of(word));
        return;
    }
    let string = view.string();
    let needle = string.substringWithRange(ns_range(&primary));
    let (ranges, _) = all(view);
    let from = ranges.iter().map(|r| r.end).max().unwrap_or(primary.end);
    let length = string.length();
    let search = |range: NSRange| string.rangeOfString_options_range(&needle, NSStringCompareOptions::empty(), range);
    let mut found = search(NSRange::new(from, length - from.min(length)));
    if found.location == NSNotFound.cast_unsigned() {
        found = search(NSRange::new(0, length));
    }
    let found = range_of(found);
    if found.start == NSNotFound.cast_unsigned() || ranges.contains(&found) {
        return;
    }
    add_caret(view, found);
}

/// Selects the block between `from` and `to`, points of the view: one selection per line, empty where the line is
/// too short or the block has no width.
pub(crate) fn column(view: &EditorTextView, from: NSPoint, to: NSPoint) {
    let (Some(first), Some(last)) = (
        super::highlight::line_at(view, from),
        super::highlight::line_at(view, to),
    ) else {
        return;
    };
    let (left, right) = (from.x.min(to.x), from.x.max(to.x));
    let lines: Vec<usize> = if first <= last {
        (first..=last).collect()
    } else {
        (last..=first).rev().collect()
    };
    let mut ranges = vec![];
    for line in lines {
        let Some(start) = view.line_start(line) else { continue };
        let Some(row) = crate::text_view::rect_of(view, start..start) else {
            continue;
        };
        let y = row.origin.y + row.size.height / 2.0;
        let a = view.characterIndexForInsertionAtPoint(NSPoint::new(left, y));
        let b = view.characterIndexForInsertionAtPoint(NSPoint::new(right, y));
        let end_of_line = view.line_start(line + 1).map_or_else(
            || view.document().text().len_utf16(),
            |next| next - newline_length(view, next),
        );
        ranges.push(a.clamp(start, end_of_line)..b.clamp(start, end_of_line));
    }
    let Some(primary) = ranges.pop() else { return };
    view.ivars().carets.borrow_mut().extra = ranges;
    select(view, &primary);
    normalize(view);
}

fn newline_length(view: &EditorTextView, next_line: usize) -> usize {
    let string = view.string();
    match (
        next_line.checked_sub(2).map(|i| string.characterAtIndex(i)),
        next_line.checked_sub(1).map(|i| string.characterAtIndex(i)),
    ) {
        (Some(0x0D), Some(0x0A)) => 2,
        (_, Some(0x0A | 0x0D)) => 1,
        _ => 0,
    }
}

/// ⌥-click adds a caret and ⌥-drag selects a column block; whether `event` was one of them.
pub(crate) fn option_mouse(view: &EditorTextView, event: &NSEvent) -> bool {
    let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
    if flags != NSEventModifierFlags::Option {
        return false;
    }
    let Some(window) = view.window() else { return false };
    let start = view.convertPoint_fromView(event.locationInWindow(), None);
    let mut dragged = false;
    while let Some(next) = window.nextEventMatchingMask(NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp) {
        if next.r#type() == NSEventType::LeftMouseUp {
            break;
        }
        let point = view.convertPoint_fromView(next.locationInWindow(), None);
        dragged |= (point.x - start.x).abs() > 3.0 || (point.y - start.y).abs() > 3.0;
        if dragged {
            view.autoscroll(&next);
            column(view, start, point);
        }
    }
    if !dragged {
        let at = view.characterIndexForInsertionAtPoint(start);
        add_caret(view, at..at);
    }
    true
}

/// The rectangles `range` takes on screen, in the view's coordinates, line by line.
pub(crate) fn rects(view: &EditorTextView, range: Range<usize>) -> Vec<NSRect> {
    let (Some(layout), Some(content)) = (view.textLayoutManager(), view.textContentStorage()) else {
        return vec![];
    };
    let Some(text_range) = crate::text_view::text_range(&content, &range) else {
        return vec![];
    };
    let origin = view.textContainerOrigin();
    let out = std::cell::RefCell::new(vec![]);
    let block = block2::RcBlock::new(
        |_: *mut objc2_app_kit::NSTextRange,
         frame: NSRect,
         _: f64,
         _: std::ptr::NonNull<objc2_app_kit::NSTextContainer>|
         -> objc2::runtime::Bool {
            crate::util::guarded(objc2::runtime::Bool::NO, || {
                out.borrow_mut().push(NSRect::new(
                    NSPoint::new(frame.origin.x + origin.x, frame.origin.y + origin.y),
                    frame.size,
                ));
                objc2::runtime::Bool::YES
            })
        },
    );
    layout.enumerateTextSegmentsInRange_type_options_usingBlock(
        &text_range,
        NSTextLayoutManagerSegmentType::Selection,
        NSTextLayoutManagerSegmentOptions::RangeNotRequired,
        &block,
    );
    drop(block);
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carets_come_in_text_order_with_the_main_one_found_again() {
        assert_eq!(merge(5..5, vec![9..9, 1..1]), (vec![1..1, 5..5, 9..9], 1));
    }

    #[test]
    fn overlapping_selections_merge_and_repeated_carets_become_one() {
        let (merged, main) = merge(2..6, vec![4..8, 12..12, 12..12]);
        assert_eq!(merged, [2..8, 12..12]);
        assert_eq!(main, 0);
    }

    #[test]
    fn selections_that_only_touch_stay_apart() {
        let touching = 3..5;
        assert_eq!(merge(0..3, vec![touching]), (vec![0..3, 3..5], 0));
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
