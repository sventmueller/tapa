//! The lines collapsed blocks hide (F78) and which lines a view shows around them: the view keeps its top where the
//! scrolling put it and shows the lines below that are not hidden, or, near the end, ends with the last line.

/// Hidden lines as sorted, disjoint ranges of global line numbers, both ends inclusive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Hidden(Vec<(u64, u64)>);

impl Hidden {
    /// The lines of `ranges`, in any order, overlapping or not.
    pub(crate) fn new(mut ranges: Vec<(u64, u64)>) -> Hidden {
        ranges.retain(|(first, last)| first <= last);
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
        for (first, last) in ranges {
            match merged.last_mut() {
                Some(previous) if first <= previous.1.saturating_add(1) => previous.1 = previous.1.max(last),
                _ => merged.push((first, last)),
            }
        }
        Hidden(merged)
    }

    /// These lines and those of `range`.
    pub(crate) fn with(&self, range: (u64, u64)) -> Hidden {
        let mut ranges = self.0.clone();
        ranges.push(range);
        Hidden::new(ranges)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The range that hides `line`.
    pub(crate) fn range_of(&self, line: u64) -> Option<(u64, u64)> {
        let at = self.0.partition_point(|&(_, last)| last < line);
        self.0.get(at).copied().filter(|&(first, _)| first <= line)
    }

    /// The first shown line at or after `line`, up to `last`.
    fn shown_from(&self, line: u64, last: u64) -> Option<u64> {
        let line = match self.range_of(line) {
            Some((_, end)) => end.checked_add(1)?,
            None => line,
        };
        (line <= last).then_some(line)
    }

    /// The last shown line at or before `line`, down to `first`.
    pub(crate) fn shown_until(&self, line: u64, first: u64) -> Option<u64> {
        let line = match self.range_of(line) {
            Some((start, _)) => start.checked_sub(1)?,
            None => line,
        };
        (line >= first).then_some(line)
    }

    /// The `count` lines a view shows, top first, between the lines `first` and `last`: the shown lines from `top` on,
    /// and where they run out before the view is full, the shown lines above.
    pub(crate) fn view(&self, top: u64, first: u64, last: u64, count: usize) -> Vec<u64> {
        let mut lines = Vec::with_capacity(count);
        let mut at = self.shown_from(top.clamp(first, last), last);
        while let Some(line) = at {
            if lines.len() == count {
                break;
            }
            lines.push(line);
            at = line.checked_add(1).and_then(|next| self.shown_from(next, last));
        }
        if lines.len() < count {
            let mut above = Vec::new();
            let start = lines.first().copied().unwrap_or(last.saturating_add(1));
            let mut at = start.checked_sub(1).and_then(|line| self.shown_until(line, first));
            while let Some(line) = at {
                if lines.len() + above.len() == count {
                    break;
                }
                above.push(line);
                at = line.checked_sub(1).and_then(|line| self.shown_until(line, first));
            }
            above.reverse();
            above.extend(lines);
            lines = above;
        }
        lines
    }

    /// The shown line `steps` shown lines from `line`, towards older lines when positive, kept between `first` and
    /// `last`.
    pub(crate) fn step(&self, line: u64, steps: isize, first: u64, last: u64) -> u64 {
        let mut at = line;
        for _ in 0..steps.unsigned_abs() {
            let next = if steps > 0 {
                at.checked_sub(1).and_then(|line| self.shown_until(line, first))
            } else {
                at.checked_add(1).and_then(|line| self.shown_from(line, last))
            };
            match next {
                Some(line) => at = line,
                None => break,
            }
        }
        at
    }
}

#[cfg(test)]
mod tests {
    use super::Hidden;

    #[test]
    fn ranges_are_sorted_and_merged() {
        let hidden = Hidden::new(vec![(10, 12), (3, 4), (5, 6), (11, 20), (9, 8)]);
        assert_eq!(hidden.0, [(3, 6), (10, 20)]);
        assert_eq!(hidden.range_of(4), Some((3, 6)));
        assert_eq!(hidden.range_of(7), None);
    }

    #[test]
    fn a_view_skips_hidden_lines_from_its_top() {
        let hidden = Hidden::new(vec![(2, 5)]);
        assert_eq!(hidden.view(0, 0, 20, 4), [0, 1, 6, 7]);
        assert_eq!(
            hidden.view(3, 0, 20, 3),
            [6, 7, 8],
            "a top inside a hidden range moves past it"
        );
    }

    #[test]
    fn near_the_end_a_view_ends_with_the_last_line() {
        let hidden = Hidden::new(vec![(5, 7)]);
        assert_eq!(hidden.view(8, 0, 9, 4), [3, 4, 8, 9]);
        assert_eq!(hidden.view(0, 0, 2, 5), [0, 1, 2], "fewer lines than the view");
    }

    #[test]
    fn steps_count_shown_lines_only() {
        let hidden = Hidden::new(vec![(5, 7)]);
        assert_eq!(hidden.step(8, 1, 0, 20), 4);
        assert_eq!(hidden.step(4, -1, 0, 20), 8);
        assert_eq!(hidden.step(2, 10, 0, 20), 0, "kept at the first line");
        assert_eq!(hidden.step(18, -10, 0, 20), 20, "kept at the last line");
    }
}
