//! What the keyboard's copy mode (F111) reads of a session and does to it: the text of every row, the history's
//! first, which rows the view shows, scrolling a row into view and selecting between two cells anywhere in the history.
//! Rows are numbered as the own core numbers them, so a number stays with its row while the history drops its oldest.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use term_vt::cell::CellFlags as VtFlags;
use term_vt::selection::{Anchor, Kind, Side};

use super::{SelectionKind, Session};

/// The text of a session's rows, the oldest of the history first, for moving over it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowsText {
    /// Each row's characters without the second half of wide ones and without trailing blanks.
    pub rows: Vec<String>,
    /// For a row whose characters and cells differ, the cell of each character.
    pub cells: HashMap<usize, Vec<usize>>,
    /// The number of the first row.
    pub first: u64,
    /// The program's cursor: an index into `rows` and a cell.
    pub cursor: (usize, usize),
    /// How many of the rows are the history's.
    pub history: usize,
}

/// Which rows a session's view shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowsView {
    /// The number of the oldest row the history keeps.
    pub first: u64,
    /// How many rows there are, the history's and the screen's.
    pub rows: usize,
    /// The index of the row at the view's top.
    pub top: usize,
    /// How many rows the view shows.
    pub lines: usize,
    /// How many cells a row has.
    pub columns: usize,
}

impl RowsView {
    /// The view line row `row` shows on, if it shows.
    pub fn line_of(&self, row: u64) -> Option<usize> {
        let index = usize::try_from(row.checked_sub(self.first)?).ok()?;
        let line = index.checked_sub(self.top)?;
        (line < self.lines).then_some(line)
    }
}

/// A row's text and, where they differ from the characters' places, the cells of its characters.
fn row_text(cells: impl Iterator<Item = (usize, char, bool, String)>) -> (String, Option<Vec<usize>>) {
    let mut text = String::new();
    let mut places = Vec::new();
    for (column, c, spacer, zero_width) in cells {
        if spacer {
            continue;
        }
        text.push(if c == '\t' { ' ' } else { c });
        places.push(column);
        for mark in zero_width.chars() {
            text.push(mark);
            places.push(column);
        }
    }
    let kept = text.trim_end_matches(' ').chars().count();
    text.truncate(text.trim_end_matches(' ').len());
    places.truncate(kept);
    let plain = places.iter().enumerate().all(|(at, column)| at == *column);
    (text, (!plain).then_some(places))
}

/// The display offset that shows the row at `index` with the least scrolling, from `offset` now.
pub(crate) fn offset_showing(index: usize, history: usize, lines: usize, offset: usize) -> usize {
    let top = history.saturating_sub(offset);
    let wanted_top = if index < top {
        index
    } else if index >= top + lines {
        index + 1 - lines
    } else {
        top
    };
    history.saturating_sub(wanted_top).min(history)
}

impl Session {
    /// The text of every row, the history's first (F111).
    pub fn rows_text(&self) -> RowsText {
        let mut out = RowsText::default();
        let mut push = |index: usize, (text, cells): (String, Option<Vec<usize>>)| {
            out.rows.push(text);
            if let Some(cells) = cells {
                out.cells.insert(index, cells);
            }
        };
        {
            let own = self.shared.own.lock();
            let grid = own.terminal.grid();
            let history = grid.history.len();
            for index in 0..history + grid.lines() {
                let Some(row) = grid.row_in_all(index) else { break };
                let cells = row.cells.iter().enumerate().map(|(column, cell)| {
                    let spacer = cell
                        .flags
                        .intersects(VtFlags(VtFlags::WIDE_SPACER.0 | VtFlags::LEADING_WIDE_SPACER.0));
                    let zero_width = row.extra(column).map(|extra| extra.zero_width.clone());
                    (column, cell.c, spacer, zero_width.unwrap_or_default())
                });
                push(index, row_text(cells));
            }
            out.first = grid.dropped;
            out.history = history;
            let cursor = own.terminal.cursor();
            out.cursor = (history + cursor.line, cursor.column);
        }
        out
    }

    /// Which rows the view shows now.
    pub fn rows_view(&self) -> RowsView {
        {
            let own = self.shared.own.lock();
            let grid = own.terminal.grid();
            let history = grid.history.len();
            RowsView {
                first: grid.dropped,
                rows: history + grid.lines(),
                top: history - grid.display_offset,
                lines: grid.lines(),
                columns: grid.columns(),
            }
        }
    }

    /// Scrolls as little as shows row `row`; whether the view moved.
    pub fn scroll_to_row(&self, row: u64) -> bool {
        let moved = {
            let mut own = self.shared.own.lock();
            let grid = own.terminal.grid_mut();
            let history = grid.history.len();
            let index = usize::try_from(row.saturating_sub(grid.dropped))
                .unwrap_or(usize::MAX)
                .min(history + grid.lines() - 1);
            let wanted = offset_showing(index, history, grid.lines(), grid.display_offset);
            let moved = wanted != grid.display_offset;
            grid.display_offset = wanted;
            moved
        };
        if moved {
            self.shared.full_redraw.store(true, Ordering::Release);
        }
        moved
    }

    /// Selects from the cell `from` to the cell `to`, each a row's number and a cell, both included; whole rows for
    /// `SelectionKind::Line`, the rectangle between them for `SelectionKind::Block`.
    pub fn select_rows(&self, kind: SelectionKind, from: (u64, usize), to: (u64, usize)) {
        {
            let mut own = self.shared.own.lock();
            let last = own.terminal.grid().columns() - 1;
            let anchor = |(line, column): (u64, usize)| Anchor {
                line,
                column: column.min(last),
            };
            let kind = match kind {
                SelectionKind::Simple => Kind::Simple,
                SelectionKind::Block => Kind::Block,
                SelectionKind::Word => Kind::Word,
                SelectionKind::Line => Kind::Line,
            };
            own.selection = Some(term_vt::selection::Selection {
                kind,
                start: (anchor(from), Side::Left),
                end: (anchor(to), Side::Right),
            });
        }
        self.shared.full_redraw.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_drops_wide_halves_and_trailing_blanks_and_keeps_their_cells() {
        let plain = [(0, 'a', false, String::new()), (1, ' ', false, String::new())];
        assert_eq!(row_text(plain.into_iter()), ("a".to_string(), None));
        let wide = [
            (0, '日', false, String::new()),
            (1, ' ', true, String::new()),
            (2, 'x', false, "\u{301}".to_string()),
            (3, ' ', false, String::new()),
        ];
        assert_eq!(
            row_text(wide.into_iter()),
            ("日x\u{301}".to_string(), Some(vec![0, 2, 2]))
        );
    }

    #[test]
    fn scrolling_to_a_row_moves_as_little_as_shows_it() {
        // 100 rows of history, a screen of 10, at the bottom: the view shows rows 100 to 109.
        assert_eq!(offset_showing(105, 100, 10, 0), 0);
        assert_eq!(offset_showing(99, 100, 10, 0), 1);
        assert_eq!(offset_showing(0, 100, 10, 0), 100);
        assert_eq!(offset_showing(45, 100, 10, 60), 60);
        assert_eq!(offset_showing(50, 100, 10, 60), 59);
        assert_eq!(offset_showing(30, 100, 10, 60), 70);
        assert_eq!(offset_showing(109, 100, 10, 60), 0);
    }
}
