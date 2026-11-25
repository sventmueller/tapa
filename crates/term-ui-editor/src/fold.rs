//! Folding: the ranges come from the indentation. A folded range's
//! lines are left out of TextKit 2's enumeration of paragraphs, so they take no room and line numbers skip them; its
//! first line stays and ends in a placeholder that unfolds it. Editing inside a fold unfolds it.

use std::ops::Range;
use std::rc::Rc;

use objc2::DefinedClass;
use objc2_app_kit::{NSTextContentManager, NSTextElement, NSTextElementProvider, NSView};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize};
use term_document::Change;

use super::view::EditorTextView;
mod indents;
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;

/// The width of a fold's placeholder after its first line.
pub(crate) const PLACEHOLDER_WIDTH: f64 = 26.0;

/// Blocks of lines as their first line and their last line, ordered by first line.
pub(crate) type Lines = Rc<[(usize, usize)]>;

/// What a view knows about folds.
#[derive(Debug, Default)]
pub(crate) struct Folds {
    /// The indentation's ranges for a revision of the document.
    indented: Option<(term_document::Revision, Lines)>,
    /// The folded ranges: UTF-16 code units from the start of the second line to the start of the line after the last
    /// hidden one, or the end of the text; sorted by start.
    hidden: Vec<Range<usize>>,
    /// Each line's indentation as far as it was looked at.
    indents: indents::Indents,
}

/// The ranges that can fold, as first line and last hidden line, ordered by first line.
fn available(view: &EditorTextView) -> Lines {
    let revision = view.document().revision();
    let mut folds = view.ivars().folds.borrow_mut();
    if let Some((seen, ranges)) = &folds.indented
        && *seen == revision
    {
        return ranges.clone();
    }
    let tab_width = tab_width(view.ivars().indent.borrow().as_str());
    let ranges: Lines = view
        .document_mut()
        .folds(tab_width)
        .into_iter()
        .filter(|f| f.end > f.start)
        .map(|f| (f.start, f.end))
        .collect();
    folds.indented = Some((revision, ranges.clone()));
    ranges
}

/// How many columns one level of `indent` takes when folding by indentation.
fn tab_width(indent: &str) -> usize {
    if indent == "\t" { 4 } else { indent.len().max(1) }
}

/// The UTF-16 offset where line `line` starts, or the end of the text past its last line.
fn line_offset(view: &EditorTextView, line: usize) -> usize {
    view.line_start(line)
        .unwrap_or_else(|| view.document().text().len_utf16())
}

/// The line UTF-16 offset `offset` lies on.
fn line_of(view: &EditorTextView, offset: usize) -> usize {
    let document = view.document();
    let text = document.text();
    text.utf16_to_byte(offset.min(text.len_utf16()))
        .map_or(0, |byte| text.line_of(byte))
}

/// Folds lines `start + 1 ..= end` under line `start`.
fn fold(view: &EditorTextView, start: usize, end: usize) {
    let hidden = line_offset(view, start + 1)..line_offset(view, end + 1);
    if hidden.is_empty() {
        return;
    }
    {
        let mut folds = view.ivars().folds.borrow_mut();
        if folds.hidden.contains(&hidden) {
            return;
        }
        folds.hidden.push(hidden.clone());
        folds.hidden.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    }
    let selection = view.selectedRange();
    if selection.location >= hidden.start && selection.location < hidden.end {
        let header_end = hidden.start.saturating_sub(newline_before(view, hidden.start));
        view.setSelectedRange(NSRange::new(header_end, 0));
    }
    relayout(view);
}

/// The length of the line break that ends just before UTF-16 offset `at`.
fn newline_before(view: &EditorTextView, at: usize) -> usize {
    let string = view.string();
    break_length(
        at.checked_sub(2).map(|i| string.characterAtIndex(i)),
        at.checked_sub(1).map(|i| string.characterAtIndex(i)),
    )
}

/// The length of the line break that the two UTF-16 units before a place end with.
fn break_length(second_last: Option<u16>, last: Option<u16>) -> usize {
    match (second_last, last) {
        (Some(0x0D), Some(0x0A)) => 2,
        (_, Some(0x0A | 0x0D)) => 1,
        _ => 0,
    }
}

/// Unfolds the folds `remove` picks; whether any went.
fn unfold_where(view: &EditorTextView, remove: impl Fn(&Range<usize>) -> bool) -> bool {
    let removed = {
        let mut folds = view.ivars().folds.borrow_mut();
        let before = folds.hidden.len();
        folds.hidden.retain(|r| !remove(r));
        folds.hidden.len() != before
    };
    if removed {
        relayout(view);
    }
    removed
}

/// Lays out the text again, so folded lines go or come back.
pub(crate) fn relayout(view: &EditorTextView) {
    super::viewport::forget(view);
    if let (Some(layout), Some(content)) = (view.textLayoutManager(), view.textContentStorage()) {
        layout.invalidateLayoutForRange(&content.documentRange());
        layout.textViewportLayoutController().layoutViewport();
    }
    view.setNeedsDisplay(true);
    redraw_fragments(view);
    super::redraw_gutter(view);
    super::overlay::redraw(view);
    super::whitespace::redraw(view);
}

/// Has every view that draws a paragraph draw again: TextKit keeps the drawing of a paragraph whose own text did not
/// change, and what it draws at the line's end moves when the fold's placeholder after it comes or goes.
fn redraw_fragments(view: &NSView) {
    for child in view.subviews().iter() {
        child.setNeedsDisplay(true);
        redraw_fragments(&child);
    }
}

/// Folds the innermost range around the cursor's line, or the one starting on it.
pub(crate) fn fold_at_cursor(view: &EditorTextView) {
    let line = line_of(view, view.selectedRange().location);
    let ranges = available(view);
    let found = ranges
        .iter()
        .filter(|(start, end)| *start <= line && line <= *end)
        .min_by_key(|(start, end)| (usize::from(*start != line), end - start));
    if let Some(&(start, end)) = found {
        fold(view, start, end);
    }
}

/// Unfolds what hides the cursor's line or starts on it.
pub(crate) fn unfold_at_cursor(view: &EditorTextView) {
    let at = view.selectedRange().location;
    let line = line_of(view, at);
    let next = line_offset(view, line + 1);
    unfold_where(view, |r| r.start == next || r.contains(&at));
}

pub(crate) fn fold_all(view: &EditorTextView) {
    for &(start, end) in available(view).iter() {
        fold(view, start, end);
    }
}

pub(crate) fn unfold_all(view: &EditorTextView) {
    unfold_where(view, |_| true);
}

/// Folds or unfolds the range that starts on `line`, as a click on its triangle does.
pub(crate) fn toggle(view: &EditorTextView, line: usize) {
    let next = line_offset(view, line + 1);
    if unfold_where(view, |r| r.start == next) {
        return;
    }
    if let Some(&(start, end)) = available(view).iter().find(|(start, _)| *start == line) {
        fold(view, start, end);
    }
}

/// Keeps folds on their lines while the text changes, and unfolds one that an edit reaches into.
pub(crate) fn shift(view: &EditorTextView, change: &Change) {
    let touched = {
        let mut folds = view.ivars().folds.borrow_mut();
        folds
            .indents
            .edited(change.start.line, change.old_end.line, change.new_end.line);
        let before = folds.hidden.len();
        folds
            .hidden
            .retain_mut(|r| !reaches_into(&change.utf16, r) && crate::text_view::follow(r, change));
        folds.hidden.len() != before
    };
    if touched {
        // The storage is still processing the edit, and TextKit must not enumerate its paragraphs meanwhile.
        view.later(objc2::sel!(termRelayoutFolds), 0.0);
    }
}

/// Whether an edit of `edited` changes text that the fold `hidden` hides.
fn reaches_into(edited: &Range<usize>, hidden: &Range<usize>) -> bool {
    edited.start > hidden.start && edited.start < hidden.end || edited.end > hidden.start && edited.start < hidden.end
}

/// Whether TextKit should lay out `element`: not when a fold hides it. A view that is busy shows everything.
pub(crate) fn shows(view: &EditorTextView, manager: &NSTextContentManager, element: &NSTextElement) -> bool {
    let Ok(folds) = view.ivars().folds.try_borrow() else {
        return true;
    };
    if folds.hidden.is_empty() {
        return true;
    }
    let Some(range) = element.elementRange() else {
        return true;
    };
    let offset = manager.offsetFromLocation_toLocation(&manager.documentRange().location(), &range.location());
    let Ok(offset) = usize::try_from(offset) else {
        return true;
    };
    !folds.hidden.iter().any(|r| r.contains(&offset))
}

/// Whether a fold hides UTF-16 offset `offset`.
pub(crate) fn hidden_by_fold(view: &EditorTextView, offset: usize) -> bool {
    view.ivars()
        .folds
        .try_borrow()
        .is_ok_and(|folds| folds.hidden.iter().any(|r| r.contains(&offset)))
}

/// The folds that are not inside another fold, as their hidden ranges.
fn outermost(view: &EditorTextView) -> Vec<Range<usize>> {
    outermost_of(&view.ivars().folds.borrow().hidden)
}

/// The ranges of `hidden`, sorted by start and longest first, that no earlier one contains.
fn outermost_of(hidden: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = vec![];
    for r in hidden {
        if !out.iter().any(|o| o.start <= r.start && r.end <= o.end) {
            out.push(r.clone());
        }
    }
    out
}

/// The placeholders of the folds on screen: where each is drawn, in the view's coordinates, with its hidden range.
pub(crate) fn placeholders(view: &EditorTextView) -> Vec<(NSRect, Range<usize>)> {
    let visible = super::visible_utf16(view);
    outermost(view)
        .into_iter()
        .filter(|r| r.start >= visible.start.saturating_sub(1) && r.start <= visible.end)
        .filter_map(|hidden| {
            let header_end = hidden.start.saturating_sub(newline_before(view, hidden.start));
            let rect = crate::text_view::rect_of(view, header_end..header_end)?;
            let at = NSRect::new(
                NSPoint::new(rect.origin.x + 4.0, rect.origin.y + 1.0),
                NSSize::new(PLACEHOLDER_WIDTH, (rect.size.height - 2.0).max(8.0)),
            );
            Some((at, hidden))
        })
        .collect()
}

/// Unfolds the fold whose placeholder lies under `point` of the view; whether there was one.
pub(crate) fn click(view: &EditorTextView, point: NSPoint) -> bool {
    let hit = placeholders(view).into_iter().find(|(rect, _)| {
        point.x >= rect.origin.x
            && point.x <= rect.origin.x + rect.size.width
            && point.y >= rect.origin.y
            && point.y <= rect.origin.y + rect.size.height
    });
    match hit {
        Some((_, hidden)) => unfold_where(view, |r| *r == hidden),
        None => false,
    }
}

/// The lines that can fold and those folded, for the line number column.
#[derive(Debug)]
pub(crate) struct Gutter {
    /// The lines on screen that start a fold by indentation.
    ranges: Lines,
    folded: Vec<usize>,
}

impl Gutter {
    /// Whether `line` is folded, if it can fold.
    pub(crate) fn get(&self, line: usize) -> Option<bool> {
        if self.folded.contains(&line) {
            return Some(true);
        }
        let at = self.ranges.partition_point(|(start, _)| *start < line);
        self.ranges
            .get(at)
            .is_some_and(|(start, _)| *start == line)
            .then_some(false)
    }
}

/// The lines among `lines` that can fold, with whether they are folded, for the line number column; the indentation
/// is looked at only around them.
pub(crate) fn gutter_within(view: &EditorTextView, lines: Range<usize>) -> Gutter {
    let ranges = {
        let tab_width = tab_width(view.ivars().indent.borrow().as_str());
        let document = view.document();
        let mut folds = view.ivars().folds.borrow_mut();
        lines
            .filter(|&line| folds.indents.folds_at(document.text(), line, tab_width))
            .map(|line| (line, line))
            .collect()
    };
    let hidden: Vec<Range<usize>> = view.ivars().folds.borrow().hidden.clone();
    let folded = hidden
        .iter()
        .map(|range| line_of(view, range.start).saturating_sub(1))
        .collect();
    Gutter { ranges, folded }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_counts_as_four_columns_and_spaces_as_themselves() {
        assert_eq!(tab_width("\t"), 4);
        assert_eq!(tab_width("  "), 2);
        assert_eq!(tab_width(""), 1, "an unknown indent still folds");
    }

    #[test]
    fn line_breaks_before_a_place_are_measured() {
        assert_eq!(break_length(Some(0x0D), Some(0x0A)), 2);
        assert_eq!(break_length(Some(u16::from(b'a')), Some(0x0A)), 1);
        assert_eq!(break_length(None, Some(0x0D)), 1);
        assert_eq!(break_length(Some(0x0A), Some(u16::from(b'a'))), 0);
        assert_eq!(break_length(None, None), 0);
    }

    #[test]
    fn edits_inside_a_fold_reach_into_it_and_edits_at_its_edges_do_not() {
        let hidden = 10..20;
        assert!(reaches_into(&(12..12), &hidden));
        assert!(reaches_into(&(5..11), &hidden));
        assert!(reaches_into(&(15..30), &hidden));
        assert!(
            !reaches_into(&(10..10), &hidden),
            "typing at the fold's start, the end of its first line"
        );
        assert!(!reaches_into(&(20..25), &hidden));
        assert!(!reaches_into(&(0..10), &hidden));
    }

    #[test]
    fn nested_folds_draw_only_the_outer_placeholder() {
        let hidden = [0..50, 10..20, 30..40, 60..70, 65..68];
        assert_eq!(outermost_of(&hidden), [0..50, 60..70]);
    }
}
