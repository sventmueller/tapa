//! Lines too long to lay out at every keystroke (SN-T7): TextKit lays out a paragraph whole, and one that does not wrap
//! sits in a container 1e7 points wide, so an editor with such a line wraps it for display.

use term_document::{Change, Text};

use super::EditorTextView;

/// As long as the longest line the syntax colouring still lexes.
const LONGEST: usize = 20_000;

/// The text of `view` is in: it wraps if a line is too long not to.
pub(super) fn opened(view: &EditorTextView) {
    if !super::opening::wraps(view) && has_long_line(view.document().text(), 0..usize::MAX, LONGEST) {
        super::opening::set_wrap(view, true);
    }
}

/// `changes` went into `view`: it wraps on the next turn if they made a line too long not to, as the storage is still
/// processing the edit.
pub(super) fn edited(view: &EditorTextView, changes: &[Change]) {
    if super::opening::wraps(view) {
        return;
    }
    let long = {
        let document = view.document();
        changes
            .iter()
            .any(|change| has_long_line(document.text(), change.start.line..change.new_end.line + 1, LONGEST))
    };
    if long {
        view.later(objc2::sel!(termWrapLongLines), 0.0);
    }
}

/// Wraps `view` if it does not yet.
pub(super) fn wrap(view: &EditorTextView) {
    if !super::opening::wraps(view) {
        super::opening::set_wrap(view, true);
    }
}

/// Whether one of `lines` of `text` is longer than `longest` bytes; it looks only at a line ending each stretch of
/// `longest` bytes, as the lines before lie wholly inside the stretch.
fn has_long_line(text: &Text, lines: std::ops::Range<usize>, longest: usize) -> bool {
    let end = lines.end.min(text.line_count());
    let mut line = lines.start;
    while line < end {
        let Some(content) = text.line_content(line) else {
            return false;
        };
        if content.len() > longest {
            return true;
        }
        line = text.line_of(content.start + longest).max(line + 1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_longer_than_the_limit_is_found_among_short_ones() {
        let short: String = (0..500).map(|n| format!("line {n}\n")).collect();
        let text = Text::new(&format!("{short}{}\n{short}", "x".repeat(101)));
        assert!(has_long_line(&text, 0..usize::MAX, 100));
        assert!(
            !has_long_line(&text, 0..usize::MAX, 101),
            "a line as long as the limit is not too long"
        );
        assert!(
            !has_long_line(&text, 0..500, 100),
            "the long line is not among the first 500"
        );
        assert!(has_long_line(&text, 500..501, 100));
    }

    #[test]
    fn a_long_last_line_without_a_break_and_a_long_first_one_are_found() {
        assert!(has_long_line(
            &Text::new(&format!("a\nb\n{}", "y".repeat(50))),
            0..usize::MAX,
            40
        ));
        assert!(has_long_line(
            &Text::new(&format!("{}\na", "y".repeat(50))),
            0..usize::MAX,
            40
        ));
        assert!(!has_long_line(&Text::new(""), 0..usize::MAX, 40));
    }

    #[test]
    fn short_lines_of_many_bytes_in_all_are_not_long() {
        let text = Text::new(&"ab\n".repeat(10_000));
        assert!(!has_long_line(&text, 0..usize::MAX, 100));
    }
}
