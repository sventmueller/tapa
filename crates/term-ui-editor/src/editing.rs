//! Typing help as the view makes it, with the edits `term_document::typing` works out: indentation that follows the
//! line above, closing brackets and quotes, commenting lines out; and going to a line.

use std::ops::Range;

use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::NSTextField;
use objc2_app_kit::NSTextInputClient;
use objc2_foundation::{NSAttributedString, NSNotFound, NSPoint, NSRange, NSRect, NSSize, NSString};
use splat_i18n::t;
use term_document::typing::{self, Step};
use term_ui::kit::alert::{Alert, Role, first};

use super::view::EditorTextView;
use crate::util::ns;

fn text_of(object: &AnyObject) -> Option<String> {
    if let Some(s) = object.downcast_ref::<NSString>() {
        return Some(s.to_string());
    }
    object
        .downcast_ref::<NSAttributedString>()
        .map(|s| s.string().to_string())
}

/// The view's text from UTF-16 offset `from` to `to`, widened to whole characters: where it starts and what it says.
fn piece(view: &EditorTextView, from: usize, to: usize) -> (usize, String) {
    let string = view.string();
    let to = to.min(string.length());
    let from = from.min(to);
    if from == to {
        return (from, String::new());
    }
    let range = string.rangeOfComposedCharacterSequencesForRange(NSRange::new(from, to - from));
    (range.location, string.substringWithRange(range).to_string())
}

/// `selection` counted from `start`, the start of a piece of the text.
fn within(selection: NSRange, start: usize) -> Range<usize> {
    selection.location - start..selection.end() - start
}

/// Makes `step`, worked out on a piece of the text that starts at UTF-16 offset `start`.
fn make(view: &EditorTextView, start: usize, step: Step) {
    let range = |r: Range<usize>| NSRange::new(r.start + start, r.end - r.start);
    match step {
        Step::Move(at) => view.setSelectedRange(NSRange::new(at + start, 0)),
        Step::Type { text, select } => {
            insert(view, &text);
            if let Some(select) = select {
                view.setSelectedRange(range(select));
            }
        }
        Step::Replace { range: replaced, text } => replace(view, range(replaced), &text),
    }
}

fn insert(view: &EditorTextView, text: &str) {
    // SAFETY: The inserted object is an NSString, one of the two types this method accepts.
    unsafe { view.insertText_replacementRange(&ns(text), NSRange::new(NSNotFound.cast_unsigned(), 0)) };
}

/// Closes brackets and quotes as they are typed, steps over a closing one already there, and wraps a selection in a
/// pair; whether it handled `object`.
pub(crate) fn typed(view: &EditorTextView, object: &AnyObject, replacement: NSRange) -> bool {
    if replacement.location != NSNotFound.cast_unsigned() || view.document().read_only().is_some() {
        return false;
    }
    let Some(text) = text_of(object) else { return false };
    let mut chars = text.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    let selection = view.selectedRange();
    let (start, piece) = piece(view, selection.location.saturating_sub(1), selection.end() + 1);
    let Some(step) = typing::typed(&piece, within(selection, start), c) else {
        return false;
    };
    make(view, start, step);
    true
}

/// A new line indented like the one above, one level deeper after an opening bracket or colon; between a bracket pair
/// the closing one moves to a line of its own.
pub(crate) fn newline(view: &EditorTextView) {
    let selection = view.selectedRange();
    let line = view.string().lineRangeForRange(NSRange::new(selection.location, 0));
    let (start, piece) = piece(view, line.location, selection.end() + 1);
    let ending = super::indent::newline(view).as_str().to_string();
    let unit = view.ivars().indent.borrow().clone();
    make(
        view,
        start,
        typing::newline(&piece, within(selection, start), &ending, &unit),
    );
}

/// Inserts one level of indentation, or indents every selected line.
pub(crate) fn tab(view: &EditorTextView) {
    let selection = view.selectedRange();
    let lines = view.string().lineRangeForRange(selection);
    let (start, piece) = piece(view, lines.location, lines.end());
    let unit = view.ivars().indent.borrow().clone();
    make(view, start, typing::tab(&piece, within(selection, start), &unit));
}

/// Comments the selected lines out, or in again when all of them are comments.
pub(crate) fn toggle_comment(view: &EditorTextView) {
    let Some(marker) = view.document().language().and_then(|l| l.line_comment()) else {
        return;
    };
    let selection = view.selectedRange();
    let lines = view.string().lineRangeForRange(selection);
    let (start, piece) = piece(view, lines.location, lines.end());
    make(
        view,
        start,
        typing::toggle_comment(&piece, within(selection, start), marker),
    );
}

/// Replaces `range` like typing would, as one undoable step.
fn replace(view: &EditorTextView, range: NSRange, text: &str) {
    if view.shouldChangeTextInRange_replacementString(range, Some(&ns(text))) {
        if let Some(storage) = view.storage() {
            storage.replaceCharactersInRange_withString(range, &ns(text));
        }
        view.didChangeText();
        view.setSelectedRange(NSRange::new(range.location, text.encode_utf16().count()));
    }
}

/// Asks for a line number and puts the cursor at its start.
pub(crate) fn go_to_line(view: &EditorTextView) {
    let mtm = view.mtm();
    let field = NSTextField::textFieldWithString(&ns(""), mtm);
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(120.0, 24.0)));
    let alert = Alert::new(mtm, t("Gehe zu Zeile"))
        .button(t("OK"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&field)
        .focus(&field);
    if !first(alert.run_modal()) {
        return;
    }
    let Ok(line) = field.stringValue().to_string().trim().parse::<usize>() else {
        return;
    };
    let count = view.document().text().line_count();
    if let Some(start) = view.line_start(line.clamp(1, count.max(1)) - 1) {
        view.setSelectedRange(NSRange::new(start, 0));
        view.scrollRangeToVisible(NSRange::new(start, 0));
    }
}
