//! The screen and its history (E4): the rows of the screen, top first, and the rows that scrolled off its top in a
//! ring buffer, the most recent last. Scrolling moves rows, never cells.

use std::borrow::Cow;
use std::ops::Range;

use crate::cell::{CellFlags, Row};
use crate::history::History;
use crate::rows::Rows;
use crate::style::StyleId;

/// The rows of one screen and its history.
#[derive(Debug, Clone)]
pub struct Grid {
    /// The visible rows, top first.
    pub rows: Rows,
    /// The rows above the screen, oldest first.
    pub history: History,
    /// The most rows the history keeps.
    pub max_history: usize,
    columns: usize,
    /// How many rows the history dropped from its top, so that a global line number stays with its row.
    pub dropped: u64,
    /// How many rows the view is scrolled back into the history.
    pub display_offset: usize,
    /// The serial the next new row gets.
    serial: u64,
}

impl Grid {
    /// An empty screen of `lines` × `columns` with room for `max_history` rows of history.
    pub fn new(lines: usize, columns: usize, max_history: usize) -> Grid {
        let mut serial = 0;
        Grid {
            rows: (0..lines)
                .map(|_| {
                    serial += 1;
                    let mut row = Row::new(columns, StyleId(0));
                    row.serial = serial;
                    row
                })
                .collect(),
            history: History::default(),
            max_history,
            columns,
            dropped: 0,
            display_offset: 0,
            serial,
        }
    }

    /// Columns per row.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// A new serial for a row written anew.
    pub fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// How many rows were written anew so far, as the serials count them.
    pub fn rows_written(&self) -> u64 {
        self.serial
    }

    pub(crate) fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
    }

    /// Visible rows.
    pub fn lines(&self) -> usize {
        self.rows.len()
    }

    /// Text moves up within `region`; at the top of the screen the rows that leave go into the history, else they
    /// go; new rows at the region's bottom are blank in `blank`.
    pub fn scroll_up(&mut self, region: Range<usize>, count: usize, blank: StyleId) {
        let count = count.min(region.len());
        if count == 0 {
            return;
        }
        if region.start == 0 && self.max_history > 0 {
            // The history keeps a copy of what each row leaving the top holds; the row comes back in as a blank one.
            for line in 0..count {
                if self.history.len() >= self.max_history {
                    self.dropped += 1;
                    self.history.drop_front();
                }
                self.history.push_back(&self.rows[line]);
            }
            if self.display_offset != 0 {
                self.display_offset = (self.display_offset + count).min(self.history.len());
            }
        }
        self.rows.rotate_up(region.clone(), count);
        self.reset_rows(region.end - count..region.end, blank);
    }

    /// Text moves down within `region`; new rows at the region's top are blank in `blank`.
    pub fn scroll_down(&mut self, region: Range<usize>, count: usize, blank: StyleId) {
        let count = count.min(region.len());
        self.rows.rotate_down(region.clone(), count);
        self.reset_rows(region.start..region.start + count, blank);
    }

    /// Blanks the rows of `range` in `blank`.
    pub fn reset_rows(&mut self, range: Range<usize>, blank: StyleId) {
        let end = range.end.min(self.rows.len());
        for index in range.start.min(end)..end {
            let serial = self.next_serial();
            let row = &mut self.rows[index];
            row.reset(blank);
            row.serial = serial;
        }
    }

    /// The global line number of the row with `serial`, if the grid still has it.
    pub fn find_serial(&self, serial: u64) -> Option<u64> {
        let index = self
            .history
            .serials()
            .chain(self.rows.iter().map(|row| row.serial))
            .position(|known| known == serial)?;
        Some(self.global(index))
    }

    /// Forgets the history.
    pub fn clear_history(&mut self) {
        self.dropped += self.history.len() as u64;
        self.history.clear();
        self.display_offset = 0;
    }

    /// The global number of the row at `line` from the top of the history.
    pub fn global(&self, line: usize) -> u64 {
        self.dropped + line as u64
    }

    /// The global number of screen line `line` as the view shows it, scrolled back or not.
    pub fn global_of_view(&self, line: usize) -> u64 {
        self.dropped + (self.history.len() - self.display_offset + line) as u64
    }

    /// The view line of global line `line`, if it shows.
    pub fn view_of_global(&self, line: u64) -> Option<usize> {
        let top = self.global_of_view(0);
        let index = usize::try_from(line.checked_sub(top)?).ok()?;
        (index < self.lines()).then_some(index)
    }

    /// The row with global number `line`, if the grid still has it.
    pub fn row_global(&self, line: u64) -> Option<Cow<'_, Row>> {
        let index = usize::try_from(line.checked_sub(self.dropped)?).ok()?;
        self.row_in_all(index)
    }

    /// The row the view shows at `line`.
    pub fn view_row(&self, line: usize) -> Option<Cow<'_, Row>> {
        self.row_in_all(self.history.len() - self.display_offset + line)
    }

    /// Scrolls the view by `lines`, positive into the history; clamped to the history there is.
    pub fn scroll_view(&mut self, lines: isize) {
        let target = isize::try_from(self.display_offset).unwrap_or(0) + lines;
        let most = isize::try_from(self.history.len()).unwrap_or(0);
        self.display_offset = usize::try_from(target.clamp(0, most)).unwrap_or(0);
    }

    /// Scrolls the screen's first `used` rows, those down to the last one that is not empty, into the history and
    /// blanks the screen, as clearing the whole screen does on the main screen.
    pub fn clear_viewport(&mut self, used: usize, blank: StyleId) {
        let lines = self.lines();
        self.scroll_up(0..lines, used, blank);
        self.reset_rows(0..lines - used, blank);
    }

    /// Row `line` counted from the top of the history, the screen's rows after it; a packed row of the history
    /// comes unpacked into a copy.
    pub fn row_in_all(&self, line: usize) -> Option<Cow<'_, Row>> {
        match line.checked_sub(self.history.len()) {
            None => self.history.get(line),
            Some(screen) => self.rows.get(screen).map(Cow::Borrowed),
        }
    }

    /// Clears the flag that row `line` wraps into the next.
    pub fn unwrap(&mut self, line: usize) {
        if let Some(row) = self.rows.get_mut(line)
            && let Some(cell) = row.cells.last_mut()
        {
            // Taking a flag away leaves only blanker cells, so the row's written extent stays right.
            cell.flags.remove(CellFlags::WRAPPED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_of(lines: &[char], history: usize) -> Grid {
        let mut grid = Grid::new(lines.len(), 2, history);
        for (line, &c) in lines.iter().enumerate() {
            let row = &mut grid.rows[line];
            row.cells[0].c = c;
            row.touch(1);
        }
        grid
    }

    fn firsts(grid: &Grid) -> String {
        grid.rows.iter().map(|row| row.cells[0].c).collect()
    }

    #[test]
    fn scrolling_up_from_the_top_keeps_history_and_rows_below_the_region() {
        let mut grid = grid_of(&['a', 'b', 'c', 'd'], 10);
        grid.scroll_up(0..3, 1, StyleId(0));
        assert_eq!(firsts(&grid), "bc d");
        assert_eq!(grid.history.len(), 1);
        assert_eq!(grid.history.get(0).unwrap().cells[0].c, 'a');
    }

    #[test]
    fn scrolling_inside_a_region_keeps_no_history() {
        let mut grid = grid_of(&['a', 'b', 'c', 'd'], 10);
        grid.scroll_up(1..3, 1, StyleId(0));
        assert_eq!(firsts(&grid), "ac d");
        grid.scroll_down(1..4, 2, StyleId(0));
        assert_eq!(firsts(&grid), "a  c");
        assert!(grid.history.is_empty());
    }

    #[test]
    fn the_history_drops_its_oldest_row_when_full() {
        let mut grid = grid_of(&['a', 'b'], 1);
        grid.scroll_up(0..2, 2, StyleId(0));
        assert_eq!(grid.history.len(), 1);
        assert_eq!(grid.history.get(0).unwrap().cells[0].c, 'b');
    }
}
