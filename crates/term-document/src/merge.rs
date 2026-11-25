//! What to do when the file changed on disk while the document had its own changes: the text both started from
//! (`base`), the document's text (`buffer`) and the file's (`disk`), merged by lines.
//!
//! Lines compare byte for byte, line breaks included, so a file that only changed its line breaks counts as changed.
//! Changes of both sides that overlap or touch, with no unchanged line between them, conflict unless they are the
//! same, as in Git.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp};

/// How long each of the two line comparisons may search before it settles for larger changes, which can only turn
/// a merge into a conflict, never into a wrong text.
const DIFF_TIME: Duration = Duration::from_millis(250);

/// What a document does about a new version of its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    /// The document had no changes of its own: it takes the file's text.
    Reload,
    /// The file holds the text the document started from, or the document's text: the document keeps its text.
    KeepBuffer,
    /// Both changed, in different places: this text holds both changes.
    Merged(String),
    /// Both changed the same places differently.
    Conflict(Conflict),
}

/// Reconciles the document's text `buffer` with the file's text `disk`, which both started from `base`.
pub fn reconcile(base: &str, buffer: &str, disk: &str) -> Reconciliation {
    if disk == buffer || disk == base {
        return Reconciliation::KeepBuffer;
    }
    if buffer == base {
        return Reconciliation::Reload;
    }
    let merge = Merge::new(base, buffer, disk);
    if merge.hunks.is_empty() {
        Reconciliation::Merged(merge.resolve(&[]))
    } else {
        Reconciliation::Conflict(Conflict(merge))
    }
}

/// A merge with places where both sides changed the same lines differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict(Merge);

/// One place where both sides changed the same lines differently, with line ranges into each of the three texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The lines of `base` both sides replaced.
    pub(crate) base_lines: Range<usize>,
    /// The lines of the document's text that replace them.
    pub(crate) buffer_lines: Range<usize>,
    /// The lines of the file's text that replace them.
    pub(crate) disk_lines: Range<usize>,
    /// The text of `base_lines`.
    pub(crate) base: String,
    /// The text of `buffer_lines`.
    pub buffer: String,
    /// The text of `disk_lines`.
    pub disk: String,
}

/// Which side a conflicting hunk takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// The document's lines.
    #[default]
    Buffer,
    /// The file's lines.
    Disk,
    /// The document's lines, then the file's.
    BufferThenDisk,
    /// The file's lines, then the document's.
    DiskThenBuffer,
}

impl Conflict {
    /// The places where the sides conflict, in the order of the text.
    pub fn hunks(&self) -> &[Hunk] {
        &self.0.hunks
    }

    /// The merged text with the side `choices` names for each hunk; a hunk without a choice keeps the document's
    /// lines.
    pub fn resolve(&self, choices: &[Choice]) -> String {
        self.0.resolve(choices)
    }

    /// The merged text with each conflicting hunk between the markers Git writes, the document's lines first under
    /// `buffer_label`, then the file's above `disk_label`, for the user to settle in the editor.
    pub fn with_markers(&self, buffer_label: &str, disk_label: &str) -> String {
        let mut out = String::new();
        for part in &self.0.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Hunk(index) => {
                    let hunk = &self.0.hunks[*index];
                    let line = |out: &mut String, text: &str| {
                        out.push_str(text);
                        if !text.is_empty() && !text.ends_with('\n') {
                            out.push('\n');
                        }
                    };
                    line(&mut out, &format!("{MARK_BUFFER} {buffer_label}"));
                    line(&mut out, &hunk.buffer);
                    line(&mut out, MARK_BETWEEN);
                    line(&mut out, &hunk.disk);
                    line(&mut out, &format!("{MARK_DISK} {disk_label}"));
                }
            }
        }
        out
    }
}

/// The marker line that opens a conflict `Conflict::with_markers` writes, up to the label.
pub(crate) const MARK_BUFFER: &str = "<<<<<<<";
/// The marker line between the two sides of a conflict.
pub(crate) const MARK_BETWEEN: &str = "=======";
/// The marker line that closes a conflict, up to the label.
pub(crate) const MARK_DISK: &str = ">>>>>>>";

/// The byte offsets of the lines of `text` that open a conflict, as `Conflict::with_markers` writes them.
pub fn conflict_markers(text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with(MARK_BUFFER) && line[MARK_BUFFER.len()..].starts_with([' ', '\n', '\r']) {
            found.push(at);
        }
        at += line.len();
    }
    found
}

/// How a line fares between two texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineChange {
    /// Both have it.
    Same,
    /// Only the old text has it.
    Removed,
    /// Only the new text has it.
    Added,
}

/// One line of a comparison, with its numbers from 1 in each text and without its line break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedLine {
    /// Which texts have it.
    pub change: LineChange,
    /// Its number in the old text.
    pub old: Option<usize>,
    /// Its number in the new text.
    pub new: Option<usize>,
    /// The line.
    pub text: String,
}

/// Where two texts differ, as runs of changed lines with up to `context` unchanged lines around each; empty for equal
/// texts.
pub fn compare(old: &str, new: &str, context: usize) -> Vec<Vec<ComparedLine>> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let deadline = Instant::now() + DIFF_TIME;
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, &old_lines, &new_lines, Some(deadline));
    let bare = |line: &str| line.trim_end_matches('\n').trim_end_matches('\r').to_string();
    similar::group_diff_ops(ops, context)
        .into_iter()
        .map(|group| {
            let mut lines = Vec::new();
            for op in group {
                let (old_range, new_range) = (op.old_range(), op.new_range());
                if matches!(op, DiffOp::Equal { .. }) {
                    for (o, n) in old_range.zip(new_range) {
                        lines.push(ComparedLine {
                            change: LineChange::Same,
                            old: Some(o + 1),
                            new: Some(n + 1),
                            text: bare(old_lines[o]),
                        });
                    }
                    continue;
                }
                for o in old_range {
                    lines.push(ComparedLine {
                        change: LineChange::Removed,
                        old: Some(o + 1),
                        new: None,
                        text: bare(old_lines[o]),
                    });
                }
                for n in new_range {
                    lines.push(ComparedLine {
                        change: LineChange::Added,
                        old: None,
                        new: Some(n + 1),
                        text: bare(new_lines[n]),
                    });
                }
            }
            lines
        })
        .collect()
}

/// A conflict over the whole text, for a file whose new version is no text to merge by lines.
pub(crate) fn whole_text_conflict(base: &str, buffer: &str, disk: &str) -> Conflict {
    let lines = |text: &str| 0..text.split_inclusive('\n').count();
    Conflict(Merge {
        parts: vec![Part::Hunk(0)],
        hunks: vec![Hunk {
            base_lines: lines(base),
            buffer_lines: lines(buffer),
            disk_lines: lines(disk),
            base: base.to_string(),
            buffer: buffer.to_string(),
            disk: disk.to_string(),
        }],
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Hunk(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Merge {
    parts: Vec<Part>,
    hunks: Vec<Hunk>,
}

/// A run of changed lines of one side: the lines of `base` it replaces and the lines of the side that replace them.
#[derive(Debug, Clone)]
struct SideEdit {
    base: Range<usize>,
    side: Range<usize>,
}

fn side_edits(base: &[&str], side: &[&str]) -> Vec<SideEdit> {
    let deadline = Instant::now() + DIFF_TIME;
    let ops = similar::capture_diff_slices_deadline(Algorithm::Myers, base, side, Some(deadline));
    let mut edits: Vec<SideEdit> = Vec::new();
    for op in ops {
        if matches!(op, DiffOp::Equal { .. }) {
            continue;
        }
        let (base_range, side_range) = (op.old_range(), op.new_range());
        match edits.last_mut() {
            Some(last) if last.base.end == base_range.start && last.side.end == side_range.start => {
                last.base.end = base_range.end;
                last.side.end = side_range.end;
            }
            _ => edits.push(SideEdit {
                base: base_range,
                side: side_range,
            }),
        }
    }
    edits
}

/// The edits of one side, taken in order, and how many lines that side is ahead of `base` behind the last one taken.
struct Side<'a> {
    lines: Vec<&'a str>,
    edits: Vec<SideEdit>,
    next: usize,
    shift: isize,
}

impl<'a> Side<'a> {
    fn new(base: &[&str], text: &'a str) -> Side<'a> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let edits = side_edits(base, &lines);
        Side {
            lines,
            edits,
            next: 0,
            shift: 0,
        }
    }

    fn next_start(&self) -> Option<usize> {
        self.edits.get(self.next).map(|edit| edit.base.start)
    }

    /// Takes the next edit if it overlaps or touches the base lines `start..end`, and returns where it ends in base.
    fn take_touching(&mut self, start: usize, end: usize) -> Option<usize> {
        let edit = self
            .edits
            .get(self.next)
            .filter(|edit| edit.base.start <= end && start <= edit.base.end)?;
        let signed = |n: usize| isize::try_from(n).unwrap_or(isize::MAX);
        self.shift = signed(edit.side.end) - signed(edit.base.end);
        self.next += 1;
        Some(edit.base.end)
    }

    /// Where base line `line` falls in this side, as far as the edits taken so far tell.
    fn at(&self, line: usize) -> usize {
        line.saturating_add_signed(self.shift)
    }

    fn text(&self, lines: Range<usize>) -> String {
        self.lines[lines].concat()
    }
}

impl Merge {
    fn new(base_text: &str, buffer_text: &str, disk_text: &str) -> Merge {
        let base: Vec<&str> = base_text.split_inclusive('\n').collect();
        let mut sides = [Side::new(&base, buffer_text), Side::new(&base, disk_text)];
        let mut parts = Vec::new();
        let mut hunks = Vec::new();
        let mut copied = 0;
        while let Some(first) = sides.iter().filter_map(Side::next_start).min() {
            parts.push(Part::Text(base[copied..first].concat()));
            let starts = [sides[0].at(first), sides[1].at(first)];
            let mut end = first;
            let mut took = [false, false];
            let mut grown = true;
            while grown {
                grown = false;
                for (side, took) in sides.iter_mut().zip(&mut took) {
                    while let Some(edit_end) = side.take_touching(first, end) {
                        end = end.max(edit_end);
                        *took = true;
                        grown = true;
                    }
                }
            }
            let [buffer_lines, disk_lines] = [0, 1].map(|n| starts[n]..sides[n].at(end));
            let buffer = sides[0].text(buffer_lines.clone());
            let disk = sides[1].text(disk_lines.clone());
            match took {
                [true, true] if buffer != disk => {
                    parts.push(Part::Hunk(hunks.len()));
                    hunks.push(Hunk {
                        base_lines: first..end,
                        buffer_lines,
                        disk_lines,
                        base: base[first..end].concat(),
                        buffer,
                        disk,
                    });
                }
                [false, true] => parts.push(Part::Text(disk)),
                _ => parts.push(Part::Text(buffer)),
            }
            copied = end;
        }
        parts.push(Part::Text(base[copied..].concat()));
        Merge { parts, hunks }
    }

    fn resolve(&self, choices: &[Choice]) -> String {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Hunk(index) => {
                    let hunk = &self.hunks[*index];
                    let (first, second) = match choices.get(*index).copied().unwrap_or_default() {
                        Choice::Buffer => (&hunk.buffer, None),
                        Choice::Disk => (&hunk.disk, None),
                        Choice::BufferThenDisk => (&hunk.buffer, Some(&hunk.disk)),
                        Choice::DiskThenBuffer => (&hunk.disk, Some(&hunk.buffer)),
                    };
                    out.push_str(first);
                    out.push_str(second.map_or("", String::as_str));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_side_decides_without_a_merge() {
        assert_eq!(reconcile("a\n", "a\n", "b\n"), Reconciliation::Reload);
        assert_eq!(reconcile("a\n", "b\n", "a\n"), Reconciliation::KeepBuffer);
        assert_eq!(reconcile("a\n", "b\n", "b\n"), Reconciliation::KeepBuffer);
        assert_eq!(reconcile("a\n", "a\n", "a\n"), Reconciliation::KeepBuffer);
    }

    #[test]
    fn changes_in_different_places_merge() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let buffer = "ONE\ntwo\nthree\nfour\nfive\n";
        let disk = "one\ntwo\nthree\nfour\nFIVE\nsix\n";
        assert_eq!(
            reconcile(base, buffer, disk),
            Reconciliation::Merged("ONE\ntwo\nthree\nfour\nFIVE\nsix\n".into())
        );
    }

    #[test]
    fn the_same_change_on_both_sides_is_no_conflict() {
        let base = "a\nb\nc\n";
        let buffer = "a\nB\nc\nmine\n";
        let disk = "a\nB\nc\n";
        assert_eq!(
            reconcile(base, buffer, disk),
            Reconciliation::Merged("a\nB\nc\nmine\n".into())
        );
    }

    #[test]
    fn changes_of_the_same_lines_conflict_and_resolve_by_choice() {
        let base = "keep\nold\nkeep too\nend\n";
        let buffer = "keep\nmine\nkeep too\nend\n";
        let disk = "KEEP\ntheirs\nkeep too\nEND\n";
        let Reconciliation::Conflict(conflict) = reconcile(base, buffer, disk) else {
            panic!("a conflict");
        };
        assert_eq!(
            conflict.hunks().len(),
            1,
            "the first line touches the conflict and joins it"
        );
        let hunk = &conflict.hunks()[0];
        assert_eq!(
            (
                hunk.base_lines.clone(),
                hunk.buffer_lines.clone(),
                hunk.disk_lines.clone()
            ),
            (0..2, 0..2, 0..2)
        );
        assert_eq!(
            (hunk.buffer.as_str(), hunk.disk.as_str()),
            ("keep\nmine\n", "KEEP\ntheirs\n")
        );
        assert_eq!(
            conflict.resolve(&[]),
            "keep\nmine\nkeep too\nEND\n",
            "the last line merged"
        );
        assert_eq!(conflict.resolve(&[Choice::Disk]), "KEEP\ntheirs\nkeep too\nEND\n");
        assert_eq!(
            conflict.resolve(&[Choice::BufferThenDisk]),
            "keep\nmine\nKEEP\ntheirs\nkeep too\nEND\n"
        );
    }

    #[test]
    fn markers_set_the_conflicting_lines_apart_as_git_does() {
        let base = "a\nold\nz";
        let Reconciliation::Conflict(conflict) = reconcile(base, "a\nmine\nz", "A\nold\ntheirs") else {
            panic!("a conflict");
        };
        let marked = conflict.with_markers("mine", "disk");
        assert_eq!(
            marked,
            "<<<<<<< mine\na\nmine\nz\n=======\nA\nold\ntheirs\n>>>>>>> disk\n"
        );
        assert_eq!(conflict_markers(&marked), [0]);
        assert_eq!(conflict_markers("x\n<<<<<<< a\ny\n<<<<<<<b\n<<<<<<<\n"), [2, 23]);
    }

    #[test]
    fn a_comparison_lists_each_changed_run_with_its_context() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\n2\nthree\n4\n5\n6\n7\n8\nnine\r\n10\n";
        let hunks = compare(old, new, 1);
        assert_eq!(hunks.len(), 2);
        let summary = |lines: &[ComparedLine]| -> Vec<(LineChange, Option<usize>, Option<usize>, String)> {
            lines.iter().map(|l| (l.change, l.old, l.new, l.text.clone())).collect()
        };
        assert_eq!(
            summary(&hunks[0]),
            [
                (LineChange::Same, Some(2), Some(2), "2".to_string()),
                (LineChange::Removed, Some(3), None, "3".to_string()),
                (LineChange::Added, None, Some(3), "three".to_string()),
                (LineChange::Same, Some(4), Some(4), "4".to_string()),
            ]
        );
        assert_eq!(
            hunks[1].last().map(|l| (l.change, l.text.as_str())),
            Some((LineChange::Added, "10"))
        );
        assert!(compare(old, old, 3).is_empty());
    }

    #[test]
    fn insertions_at_the_same_place_conflict_unless_equal() {
        let base = "a\nb\n";
        let Reconciliation::Conflict(conflict) = reconcile(base, "a\nx\nb\n", "a\ny\nb\n") else {
            panic!("a conflict");
        };
        assert_eq!(conflict.hunks()[0].base_lines, 1..1);
        assert_eq!(conflict.resolve(&[Choice::DiskThenBuffer]), "a\ny\nx\nb\n");
    }

    #[test]
    fn a_change_of_line_breaks_alone_counts_as_a_change() {
        let base = "a\nb\nc\nd\n";
        let buffer = "a\nb\nc\nD\n";
        let disk = "a\r\nb\nc\nd\n";
        assert_eq!(
            reconcile(base, buffer, disk),
            Reconciliation::Merged("a\r\nb\nc\nD\n".into())
        );
    }

    #[test]
    fn merging_finds_the_same_text_as_applying_both_sides_in_order() {
        let base: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let lines: Vec<&str> = base.split_inclusive('\n').collect();
        let edit = |text: &[&str], at: usize, with: &str| -> String {
            let mut out: Vec<String> = text.iter().map(|s| s.to_string()).collect();
            out[at] = with.to_string();
            out.concat()
        };
        for (a, b) in [(3, 10), (10, 3), (0, 39), (20, 22)] {
            let buffer = edit(&lines, a, "buffer\n");
            let disk = edit(&lines, b, "disk\n");
            let both = edit(
                &edit(&lines, a, "buffer\n").split_inclusive('\n').collect::<Vec<_>>(),
                b,
                "disk\n",
            );
            assert_eq!(
                reconcile(&base, &buffer, &disk),
                Reconciliation::Merged(both),
                "{a} {b}"
            );
        }
    }
}
