//! What the scrollbar marks along the history (F106): prompts, search matches and failed commands, at the rows they
//! are on, numbered as `RowsView` numbers rows.

use term_vt::term::{Mark, MarkKind};

use super::{RowsView, Session};

/// What a mark on the scrollbar stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScrollMark {
    /// A prompt began.
    Prompt,
    /// A search match.
    Hit,
    /// A command that ended with an exit code other than 0, at its prompt.
    Failed,
}

/// The rows of a session and the marks among them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrollMarks {
    /// Which rows there are and which of them show.
    pub view: RowsView,
    /// Each mark with its row's number, in no particular order.
    pub marks: Vec<(u64, ScrollMark)>,
}

impl ScrollMarks {
    /// Adds `mark` at row `row`, for whoever knows of more, such as the command blocks.
    pub fn push(&mut self, row: u64, mark: ScrollMark) {
        self.marks.push((row, mark));
    }

    /// Where row `row` lies along the rows, from 0 at the oldest to 1 at the newest; none for a row that is gone.
    pub fn fraction(&self, row: u64) -> Option<f64> {
        let index = row.checked_sub(self.view.first)?;
        let rows = self.view.rows as u64;
        (index < rows).then(|| index as f64 / rows.max(1) as f64)
    }
}

/// The prompts and the failed commands among the shell integration's marks, oldest first; a failed command is marked
/// at the prompt it was typed at.
pub(crate) fn from_shell(marks: &[(u64, &Mark)]) -> Vec<(u64, ScrollMark)> {
    let mut found = Vec::new();
    let mut prompt = None;
    for &(row, mark) in marks {
        match mark.kind {
            MarkKind::Prompt => {
                prompt = Some(row);
                found.push((row, ScrollMark::Prompt));
            }
            MarkKind::Finished(Some(code)) if code != 0 => {
                found.push((prompt.unwrap_or(row), ScrollMark::Failed));
            }
            _ => {}
        }
    }
    found
}

impl Session {
    /// The rows and what the scrollbar marks among them.
    pub fn scroll_marks(&self) -> ScrollMarks {
        let view = self.rows_view();
        let marks = {
            let own = self.shared.own.lock();
            let mut marks = from_shell(&own.terminal.marks());
            marks.extend(own.search_lines().into_iter().map(|row| (row, ScrollMark::Hit)));
            marks
        };
        ScrollMarks { view, marks }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(kind: MarkKind) -> Mark {
        Mark::new(kind, 0, 0)
    }

    #[test]
    fn a_failed_command_is_marked_at_its_prompt() {
        let marks = [
            mark(MarkKind::Prompt),
            mark(MarkKind::Finished(Some(0))),
            mark(MarkKind::Prompt),
            mark(MarkKind::Output(None)),
            mark(MarkKind::Finished(Some(2))),
            mark(MarkKind::Finished(None)),
        ];
        let rows: Vec<(u64, &Mark)> = [3, 5, 6, 7, 9, 9].into_iter().zip(&marks).collect();
        assert_eq!(
            from_shell(&rows),
            [
                (3, ScrollMark::Prompt),
                (6, ScrollMark::Prompt),
                (6, ScrollMark::Failed)
            ]
        );
    }

    #[test]
    fn a_row_lies_along_the_rows_the_history_still_keeps() {
        let marks = ScrollMarks {
            view: RowsView {
                first: 100,
                rows: 50,
                top: 0,
                lines: 10,
                columns: 80,
            },
            marks: vec![],
        };
        assert_eq!(marks.fraction(99), None);
        assert_eq!(marks.fraction(100), Some(0.0));
        assert_eq!(marks.fraction(125), Some(0.5));
        assert_eq!(marks.fraction(150), None);
    }
}
