//! Edits as AppKit makes them, what each did in the units of its consumers, and the fewest edits from one text to
//! another.

use std::fmt;
use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp};

use crate::disk::ReadOnly;
use crate::document::Revision;
use crate::text::{Point, PositionError};

/// How long the line comparison behind `Document::replace_all` may search before it settles for larger edits.
const DIFF_TIME: Duration = Duration::from_millis(100);

/// A replacement of a range of the text by `text`, in code units of UTF-16 as `NSTextView` counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The range replaced, in the text before the edit.
    pub range_utf16: std::ops::Range<usize>,
    /// What replaces it.
    pub text: String,
}

/// What one edit did, for every consumer: ranges and the points `start` and `old_end` refer to the text before it,
/// `new_end` to the text after it, so a language server's incremental change is `start`..`old_end` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The document's revision after the edit.
    pub revision: Revision,
    /// The bytes replaced.
    pub bytes: Range<usize>,
    /// The UTF-16 code units replaced.
    pub utf16: Range<usize>,
    /// Where the replaced range starts.
    pub start: Point,
    /// Where the replaced range ended.
    pub old_end: Point,
    /// Where the new text ends.
    pub new_end: Point,
    /// The new text's length in UTF-16 code units.
    pub text_utf16: usize,
    /// The new text.
    pub text: String,
}

impl Change {
    /// The bytes the new text takes in the text after the edit.
    pub fn new_bytes(&self) -> Range<usize> {
        self.bytes.start..self.bytes.start + self.text.len()
    }

    /// The UTF-16 code units the new text takes in the text after the edit.
    pub fn new_utf16(&self) -> Range<usize> {
        self.utf16.start..self.utf16.start + self.text_utf16
    }
}

/// Why an edit was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// The document is read-only, for this reason.
    ReadOnly(ReadOnly),
    /// The range names no place in the text.
    Position(PositionError),
}

impl From<PositionError> for EditError {
    fn from(error: PositionError) -> EditError {
        EditError::Position(error)
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::ReadOnly(why) => write!(formatter, "the document is read-only: {why:?}"),
            EditError::Position(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for EditError {}

/// A replacement of the bytes `range` of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ByteEdit {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
}

/// Edits that turn `old` into `new`, from the end of the text to its start, so each keeps its offsets in `old`. Lines
/// that stay are left alone, and a changed run of lines shrinks to what differs between its first and last byte.
pub(crate) fn edits_between(old: &str, new: &str) -> Vec<ByteEdit> {
    if old == new {
        return Vec::new();
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let old_starts = line_starts(&old_lines);
    let new_starts = line_starts(&new_lines);
    let deadline = Instant::now() + DIFF_TIME;
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, &old_lines, &new_lines, Some(deadline));

    let mut runs: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for op in ops {
        if matches!(op, DiffOp::Equal { .. }) {
            continue;
        }
        let (old_range, new_range) = (op.old_range(), op.new_range());
        match runs.last_mut() {
            Some((o, n)) if o.end == old_range.start && n.end == new_range.start => {
                o.end = old_range.end;
                n.end = new_range.end;
            }
            _ => runs.push((old_range, new_range)),
        }
    }
    let mut edits: Vec<ByteEdit> = runs
        .into_iter()
        .map(|(o, n)| {
            let old_bytes = old_starts[o.start]..old_starts[o.end];
            let new_bytes = new_starts[n.start]..new_starts[n.end];
            narrowed(old, old_bytes, &new[new_bytes])
        })
        .collect();
    edits.reverse();
    edits
}

fn line_starts(lines: &[&str]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(lines.len() + 1);
    let mut at = 0;
    starts.push(0);
    for line in lines {
        at += line.len();
        starts.push(at);
    }
    starts
}

/// The edit that replaces `range` of `old` by `new`, without the bytes both share at its start and end.
fn narrowed(old: &str, range: Range<usize>, new: &str) -> ByteEdit {
    let before = &old[range.clone()];
    let mut prefix = before.bytes().zip(new.bytes()).take_while(|(a, b)| a == b).count();
    while !before.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let (before_rest, new_rest) = (&before[prefix..], &new[prefix..]);
    let mut suffix = before_rest
        .bytes()
        .rev()
        .zip(new_rest.bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !before_rest.is_char_boundary(before_rest.len() - suffix) {
        suffix -= 1;
    }
    ByteEdit {
        range: range.start + prefix..range.end - suffix,
        text: new_rest[..new_rest.len() - suffix].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, edits: &[ByteEdit]) -> String {
        let mut text = old.to_string();
        for edit in edits {
            text.replace_range(edit.range.clone(), &edit.text);
        }
        text
    }

    #[test]
    fn edits_between_two_texts_touch_only_what_differs() {
        let old = "fn a() {\n    one();\n}\n\nfn b() {}\n";
        let new = "fn a() {\n    two();\n}\n\nfn b() {}\nfn c() {}\n";
        let edits = edits_between(old, new);
        assert_eq!(
            edits,
            [
                ByteEdit {
                    range: 33..33,
                    text: "fn c() {}\n".into()
                },
                ByteEdit {
                    range: 13..16,
                    text: "two".into()
                },
            ]
        );
        assert_eq!(apply(old, &edits), new);
    }

    #[test]
    fn identical_texts_need_no_edit_and_different_ones_always_arrive() {
        assert!(edits_between("same\n", "same\n").is_empty());
        for (old, new) in [
            ("", "x"),
            ("x", ""),
            ("a\nb", "a\nb\n"),
            ("ä\n", "ö\n"),
            ("a\r\nb\r\n", "a\nb\n"),
            ("😀😁", "😀😂"),
            ("one\ntwo\nthree\n", "three\ntwo\none\n"),
        ] {
            let edits = edits_between(old, new);
            assert_eq!(apply(old, &edits), new, "{old:?} -> {new:?}");
            for edit in &edits {
                assert!(old.is_char_boundary(edit.range.start) && old.is_char_boundary(edit.range.end));
            }
            assert!(
                edits.windows(2).all(|w| w[1].range.end <= w[0].range.start),
                "from the end"
            );
        }
    }
}
