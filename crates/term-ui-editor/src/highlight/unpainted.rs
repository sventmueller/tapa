//! The lines whose colours the view does not show yet.

use std::ops::Range;

/// Lines whose colours wait to be painted, as sorted ranges that neither overlap nor touch.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Unpainted(Vec<Range<usize>>);

impl Unpainted {
    pub(super) fn add(&mut self, lines: Range<usize>) {
        if lines.is_empty() {
            return;
        }
        let at = self.0.partition_point(|r| r.end < lines.start);
        let mut merged = lines;
        while let Some(next) = self.0.get(at).filter(|r| r.start <= merged.end) {
            merged = merged.start.min(next.start)..merged.end.max(next.end);
            self.0.remove(at);
        }
        self.0.insert(at, merged);
    }

    /// Lines `first..=old_end` became `first..=new_end`: the ranges below move with them, the edited lines join.
    pub(super) fn edited(&mut self, first: usize, old_end: usize, new_end: usize) {
        let moved = |line: usize, edited: usize| {
            if line < first {
                line
            } else if line > old_end {
                line - old_end + new_end
            } else {
                edited
            }
        };
        let ranges = std::mem::take(&mut self.0);
        for range in ranges {
            self.add(moved(range.start, first)..moved(range.end - 1, new_end) + 1);
        }
        self.add(first..new_end + 1);
    }

    /// Takes out the lines of `lines` and returns the stretch they span.
    pub(super) fn take_in(&mut self, lines: &Range<usize>) -> Option<Range<usize>> {
        let mut taken: Option<Range<usize>> = None;
        let mut kept = Vec::with_capacity(self.0.len() + 1);
        for range in std::mem::take(&mut self.0) {
            let (start, end) = (range.start.max(lines.start), range.end.min(lines.end));
            if start >= end {
                kept.push(range);
                continue;
            }
            taken = Some(taken.map_or(start..end, |t| t.start.min(start)..t.end.max(end)));
            kept.extend(
                [range.start..start, end..range.end]
                    .into_iter()
                    .filter(|r| !r.is_empty()),
            );
        }
        self.0 = kept;
        taken
    }

    /// Whether any of `lines` waits.
    pub(super) fn meets(&self, lines: &Range<usize>) -> bool {
        let at = self.0.partition_point(|r| r.end <= lines.start);
        self.0.get(at).is_some_and(|r| r.start < lines.end)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::Unpainted;

    fn one(lines: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
        std::iter::once(lines).collect()
    }

    #[test]
    fn unpainted_lines_merge_where_they_meet_and_give_up_what_is_taken() {
        let mut lines = Unpainted::default();
        lines.add(10..20);
        lines.add(30..40);
        lines.add(20..25);
        assert_eq!(lines.0, [10..25, 30..40]);
        assert!(lines.meets(&(24..26)) && lines.meets(&(0..11)) && lines.meets(&(39..50)));
        assert!(!lines.meets(&(25..30)) && !lines.meets(&(0..10)) && !lines.meets(&(40..50)));
        lines.add(5..31);
        assert_eq!(lines.0, one(5..40));
        assert_eq!(lines.take_in(&(12..18)), Some(12..18));
        assert_eq!(lines.0, [5..12, 18..40]);
        assert_eq!(lines.take_in(&(0..100)), Some(5..40));
        assert!(lines.is_empty());
    }

    #[test]
    fn unpainted_lines_follow_an_edit_and_take_in_its_lines() {
        let mut lines = Unpainted(vec![2..4, 50..60]);
        lines.edited(10, 10, 12);
        assert_eq!(lines.0, [2..4, 10..13, 52..62], "two lines more below line 10");
        let mut lines = Unpainted(one(50..60));
        lines.edited(10, 15, 10);
        assert_eq!(lines.0, [10..11, 45..55], "five lines fewer");
        let mut lines = Unpainted(one(8..14));
        lines.edited(10, 12, 10);
        assert_eq!(
            lines.0,
            one(8..12),
            "a range through the edit keeps its start and ends after what is left"
        );
    }
}
