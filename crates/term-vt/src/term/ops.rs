//! What the terminal does for each operation: printing, moving, scrolling, erasing, inserting and deleting.

use unicode_width::UnicodeWidthChar;

use super::{Cursor, Modes, Terminal, tab_stops};
use crate::cell::{Cell, CellFlags};
use crate::style::{Style, StyleId};

impl Terminal {
    fn last_column(&self) -> usize {
        self.size.columns - 1
    }

    /// The cursor's style as a cell's style number, looked up once until the style changes.
    fn cursor_style(&mut self) -> StyleId {
        if let Some(id) = self.cursor_style_id {
            return id;
        }
        let style = self.cursor.style;
        let id = self.style_id(style);
        self.cursor_style_id = Some(id);
        id
    }

    /// Prints a run of printable ASCII: as many characters as fit on the line are written in one go where nothing
    /// asks for more care (insert mode, a character set, double-width characters, a link), the rest one by one.
    pub(super) fn input_ascii(&mut self, mut run: &[u8]) {
        while !run.is_empty() {
            let simple = !self.cursor.wrap_pending
                && !self.modes.contains(Modes::INSERT)
                && self.cursor.charsets[self.active_charset] == super::Charset::Ascii
                && self.cursor.link.is_none();
            if !simple {
                self.input(char::from(run[0]));
                run = &run[1..];
                continue;
            }
            let columns = self.size.columns;
            let (line, column) = (self.cursor.line, self.cursor.column);
            let room = columns - column;
            let count = run.len().min(room);
            let style = self.cursor_style();
            let row = &mut self.grid.rows[line];
            let wide = row.cells[column..column + count]
                .iter()
                .any(|cell| cell.flags.0 & (CellFlags::WIDE.0 | CellFlags::WIDE_SPACER.0) != 0)
                || (column + count < columns && row.cells[column + count].flags.contains(CellFlags::WIDE_SPACER));
            if wide {
                self.input(char::from(run[0]));
                run = &run[1..];
                continue;
            }
            if !row.extras.is_empty() {
                row.extras
                    .retain(|(at, _)| !(column..column + count).contains(&usize::from(*at)));
            }
            row.touch(column + count);
            for (cell, &byte) in row.cells[column..column + count].iter_mut().zip(run) {
                *cell = Cell {
                    c: char::from(byte),
                    style,
                    flags: CellFlags::default(),
                };
            }
            run = &run[count..];
            if column + count < columns {
                self.cursor.column = column + count;
            } else {
                self.cursor.column = columns - 1;
                self.cursor.wrap_pending = true;
            }
        }
    }

    /// Prints a run of characters beyond ASCII: as many as fit on the line straight into their cells where nothing
    /// asks for more care, as Ghostty's `printSlice` does, the rest one by one.
    pub(super) fn input_chars(&mut self, run: &[char]) {
        let mut at = 0;
        while at < run.len() {
            let placed = self.place_chars(&run[at..]);
            if placed == 0 {
                self.input(run[at]);
                at += 1;
            } else {
                at += placed;
            }
        }
    }

    /// Writes the leading characters of `run` of width 1 or 2 into the cursor's line while the cells they cover hold
    /// no wide character, extra or spacer; how many it wrote.
    fn place_chars(&mut self, run: &[char]) -> usize {
        if self.cursor.wrap_pending || self.modes.contains(Modes::INSERT) || self.cursor.link.is_some() {
            return 0;
        }
        let columns = self.size.columns;
        let style = self.cursor_style();
        let busy = CellFlags(CellFlags::WIDE.0 | CellFlags::WIDE_SPACER.0 | CellFlags::EXTRA.0);
        let start = self.cursor.column;
        let row = &mut self.grid.rows[self.cursor.line];
        let mut column = start;
        let mut placed = 0;
        // Past what the row holds every cell is blank, so nothing there needs a look.
        let blank_from = row.stored().0;
        for &c in run {
            let width = char_width(c).unwrap_or(0);
            if !(1..=2).contains(&width) || column + width > columns {
                break;
            }
            if column < blank_from
                && (row.cells[column..column + width]
                    .iter()
                    .any(|cell| cell.flags.intersects(busy))
                    || row
                        .cells
                        .get(column + width)
                        .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER)))
            {
                break;
            }
            if width == 1 {
                row.cells[column] = Cell {
                    c,
                    style,
                    flags: CellFlags::default(),
                };
            } else {
                row.cells[column] = Cell {
                    c,
                    style,
                    flags: CellFlags::WIDE,
                };
                row.cells[column + 1] = Cell {
                    c: ' ',
                    style,
                    flags: CellFlags::WIDE_SPACER,
                };
            }
            column += width;
            placed += 1;
            if column == columns {
                break;
            }
        }
        if placed == 0 {
            return 0;
        }
        row.touch(column);
        if column < columns {
            self.cursor.column = column;
        } else {
            self.cursor.column = columns - 1;
            self.cursor.wrap_pending = true;
        }
        placed
    }

    /// Prints `c` at the cursor, wrapping first if the cursor waits at the end of a line.
    pub(super) fn input(&mut self, c: char) {
        let Some(width) = char_width(c) else { return };
        if width == 0 {
            return self.input_zero_width(c);
        }
        if self.input_plain(c, width) {
            return;
        }
        if self.cursor.wrap_pending {
            // Printing uses the pending wrap up even where autowrap is off and nothing wraps, as xterm's dotext does.
            self.cursor.wrap_pending = false;
            self.wrapline();
        }
        let columns = self.size.columns;
        if self.modes.contains(Modes::INSERT) && self.cursor.column + width < columns {
            let blank = self.blank();
            let row = &mut self.grid.rows[self.cursor.line];
            for column in (self.cursor.column..columns - width).rev() {
                row.swap(column + width, column);
            }
            // The cells the shift left at the cursor came from the line's end, and the shift may have split a wide
            // character there or at the cursor.
            let column = self.cursor.column;
            row.blank(column..column + width, blank);
            row.mend_wide(&[column.wrapping_sub(1), column + width, columns - 1], blank);
        }
        if width == 1 {
            self.write_at_cursor(c, CellFlags::default());
        } else {
            if self.cursor.column + 1 >= columns {
                if !self.modes.contains(Modes::LINE_WRAP) {
                    return;
                }
                self.write_at_cursor(' ', CellFlags::LEADING_WIDE_SPACER);
                self.wrapline();
            }
            self.write_at_cursor(c, CellFlags::WIDE);
            self.cursor.column += 1;
            self.write_at_cursor(' ', CellFlags::WIDE_SPACER);
        }
        if self.cursor.column + 1 < columns {
            self.cursor.column += 1;
        } else {
            self.cursor.wrap_pending = true;
        }
    }

    /// Writes a character beyond ASCII of `width` 1 or 2 straight into its cells where nothing asks for more care,
    /// as `input_ascii` does for ASCII; whether it did. The character sets change only ASCII.
    fn input_plain(&mut self, c: char, width: usize) -> bool {
        let (line, column) = (self.cursor.line, self.cursor.column);
        let columns = self.size.columns;
        if c.is_ascii()
            || !(1..=2).contains(&width)
            || self.cursor.wrap_pending
            || column + width > columns
            || self.modes.contains(Modes::INSERT)
            || self.cursor.link.is_some()
        {
            return false;
        }
        let busy = CellFlags(CellFlags::WIDE.0 | CellFlags::WIDE_SPACER.0 | CellFlags::EXTRA.0);
        let row = &self.grid.rows[line];
        if row.cells[column..column + width]
            .iter()
            .any(|cell| cell.flags.intersects(busy))
            || row
                .cells
                .get(column + width)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
        {
            return false;
        }
        let style = self.cursor_style();
        let row = &mut self.grid.rows[line];
        if width == 1 {
            row.cells[column] = Cell {
                c,
                style,
                flags: CellFlags::default(),
            };
        } else {
            row.cells[column] = Cell {
                c,
                style,
                flags: CellFlags::WIDE,
            };
            row.cells[column + 1] = Cell {
                c: ' ',
                style,
                flags: CellFlags::WIDE_SPACER,
            };
        }
        row.touch(column + width);
        if column + width < columns {
            self.cursor.column = column + width;
        } else {
            self.cursor.column = columns - 1;
            self.cursor.wrap_pending = true;
        }
        true
    }

    /// A character of no width joins the cell before the cursor, or the wide character that cell belongs to.
    fn input_zero_width(&mut self, c: char) {
        let mut column = self.cursor.column;
        if !self.cursor.wrap_pending {
            column = column.saturating_sub(1);
        }
        let row = &mut self.grid.rows[self.cursor.line];
        if row.cells[column].flags.contains(CellFlags::WIDE_SPACER) {
            column = column.saturating_sub(1);
        }
        row.extra_mut(column).zero_width.push(c);
    }

    /// Writes `c` with the cursor's style and `flags` into the cell at the cursor, clearing what a double-width
    /// character there leaves.
    fn write_at_cursor(&mut self, c: char, flags: CellFlags) {
        let c = self.cursor.charsets[self.active_charset].map(c);
        let style = self.cursor_style();
        let link = self.cursor.link.clone();
        let (line, column) = (self.cursor.line, self.cursor.column);
        let last = self.last_column();
        let top = self.grid.history.is_empty() && line == 0;
        let here = self.grid.rows[line].cells[column].flags;
        if here.intersects(CellFlags(CellFlags::WIDE.0 | CellFlags::WIDE_SPACER.0)) {
            // The other half goes blank in the current background, as in xterm and Ghostty.
            let blank = self.blank();
            let row = &mut self.grid.rows[line];
            if here.contains(CellFlags::WIDE) && column < last {
                row.blank(column + 1..column + 2, blank);
            } else if column > 0 && here.contains(CellFlags::WIDE_SPACER) {
                row.blank(column - 1..column, blank);
            }
            if column <= 1 && !top {
                match line.checked_sub(1) {
                    Some(above) => {
                        let above = &mut self.grid.rows[above];
                        above.cells[last].flags.remove(CellFlags::LEADING_WIDE_SPACER);
                        above.touch(last + 1);
                    }
                    None => self.grid.history.clear_last_flag(CellFlags::LEADING_WIDE_SPACER),
                }
            }
        }
        let row = &mut self.grid.rows[line];
        row.clear_extra(column);
        row.cells[column] = Cell { c, style, flags };
        row.touch(column + 1);
        if let Some(link) = link {
            row.extra_mut(column).link = Some(link);
        }
    }

    /// Moves to the start of the next line, marking the line as wrapped.
    pub(super) fn wrapline(&mut self) {
        if !self.modes.contains(Modes::LINE_WRAP) {
            return;
        }
        let (line, column) = (self.cursor.line, self.cursor.column);
        let row = &mut self.grid.rows[line];
        row.cells[column].flags.insert(CellFlags::WRAPPED);
        row.touch(column + 1);
        if line + 1 >= self.region.end {
            self.linefeed();
        } else {
            self.cursor.line += 1;
        }
        self.cursor.column = 0;
        self.cursor.wrap_pending = false;
    }

    pub(super) fn linefeed(&mut self) {
        self.cursor.wrap_pending = false;
        let next = self.cursor.line + 1;
        if next == self.region.end {
            self.scroll_up(1);
        } else if next < self.size.lines {
            self.cursor.line += 1;
        }
    }

    pub(super) fn carriage_return(&mut self) {
        self.cursor.column = 0;
        self.cursor.wrap_pending = false;
    }

    pub(super) fn backspace(&mut self) {
        self.cursor.column = self.cursor.column.saturating_sub(1);
        self.cursor.wrap_pending = false;
    }

    pub(super) fn put_tab(&mut self, mut count: u16) {
        let columns = self.size.columns;
        while self.cursor.column < columns && count != 0 {
            count -= 1;
            let c = self.cursor.charsets[self.active_charset].map('\t');
            let row = &mut self.grid.rows[self.cursor.line];
            if row.cells[self.cursor.column].c == ' ' {
                row.cells[self.cursor.column].c = c;
                row.touch(self.cursor.column + 1);
            }
            loop {
                if self.cursor.column + 1 == columns {
                    break;
                }
                self.cursor.column += 1;
                if self.tabs[self.cursor.column] {
                    break;
                }
            }
        }
    }

    pub(super) fn move_forward_tabs(&mut self, count: u16) {
        let columns = self.size.columns;
        for _ in 0..count {
            let mut column = self.cursor.column;
            if column == columns - 1 {
                break;
            }
            for next in column + 1..columns {
                column = next;
                if self.tabs[column] {
                    break;
                }
            }
            self.cursor.column = column;
        }
    }

    pub(super) fn move_backward_tabs(&mut self, count: u16) {
        for _ in 0..count {
            let column = self.cursor.column;
            if column == 0 {
                break;
            }
            self.cursor.column = (0..column).rev().find(|&before| self.tabs[before]).unwrap_or(0);
        }
    }

    /// Moves to `line` and `column`, counted from the region's top in origin mode.
    pub(super) fn goto(&mut self, line: i64, column: usize) {
        let (offset, last) = if self.modes.contains(Modes::ORIGIN) {
            (self.region.start, self.region.end - 1)
        } else {
            (0, self.size.lines - 1)
        };
        let line = (line + i64::try_from(offset).unwrap_or(0)).clamp(0, i64::try_from(last).unwrap_or(0));
        self.cursor.line = usize::try_from(line).unwrap_or(0);
        self.cursor.column = column.min(self.last_column());
        self.cursor.wrap_pending = false;
    }

    pub(super) fn goto_line(&mut self, line: i64) {
        self.goto(line, self.cursor.column);
    }

    pub(super) fn goto_column(&mut self, column: usize) {
        self.cursor.column = column.min(self.last_column());
        self.cursor.wrap_pending = false;
    }

    /// The first line `goto` counts from: the region's top in origin mode.
    fn origin(&self) -> usize {
        if self.modes.contains(Modes::ORIGIN) {
            self.region.start
        } else {
            0
        }
    }

    /// CUU: up, stopping at the region's top when the cursor starts inside or below it, as xterm does.
    pub(super) fn move_up(&mut self, lines: usize) {
        let top = if self.cursor.line >= self.region.start {
            self.region.start
        } else {
            0
        };
        self.cursor.line = self.cursor.line.saturating_sub(lines).max(top);
        self.cursor.wrap_pending = false;
    }

    /// CUD: down, stopping at the region's bottom when the cursor starts inside or above it, as xterm does.
    pub(super) fn move_down(&mut self, lines: usize) {
        let bottom = if self.cursor.line < self.region.end {
            self.region.end - 1
        } else {
            self.size.lines - 1
        };
        self.cursor.line = (self.cursor.line + lines).min(bottom);
        self.cursor.wrap_pending = false;
    }

    /// VPR: down from the cursor's line as an absolute move would count it, past the region's bottom.
    pub(super) fn move_down_absolute(&mut self, lines: usize) {
        let line = i64::try_from(self.cursor.line.saturating_sub(self.origin()) + lines).unwrap_or(i64::MAX);
        self.goto(line, self.cursor.column);
    }

    pub(super) fn move_forward(&mut self, columns: usize) {
        self.cursor.column = (self.cursor.column + columns).min(self.last_column());
        self.cursor.wrap_pending = false;
    }

    pub(super) fn move_backward(&mut self, columns: usize) {
        self.cursor.column = self.cursor.column.saturating_sub(columns);
        self.cursor.wrap_pending = false;
    }

    pub(super) fn scroll_up(&mut self, lines: usize) {
        let origin = self.region.start;
        self.scroll_up_from(origin, lines);
    }

    pub(super) fn scroll_down(&mut self, lines: usize) {
        let origin = self.region.start;
        self.scroll_down_from(origin, lines);
    }

    /// Text from `origin` to the region's bottom moves up by `lines`.
    fn scroll_up_from(&mut self, origin: usize, lines: usize) {
        let lines = lines.min(self.region.len());
        let blank = self.blank_row_style();
        let end = self.region.end;
        self.grid.scroll_up(origin..end, lines, blank);
    }

    /// Text from `origin` to the region's bottom moves down by `lines`.
    fn scroll_down_from(&mut self, origin: usize, lines: usize) {
        let lines = lines.min(self.region.len()).min(self.region.end - origin);
        let blank = self.blank_row_style();
        let end = self.region.end;
        self.grid.scroll_down(origin..end, lines, blank);
    }

    /// The style of rows that scrolling brings in: blank in the cursor's background.
    fn blank_row_style(&mut self) -> StyleId {
        self.blank()
    }

    pub(super) fn insert_blank_lines(&mut self, lines: usize) {
        let origin = self.cursor.line;
        if self.region.contains(&origin) {
            self.scroll_down_from(origin, lines);
            self.carriage_return();
        }
    }

    pub(super) fn delete_lines(&mut self, lines: usize) {
        let origin = self.cursor.line;
        let lines = (self.size.lines - origin).min(lines);
        if lines > 0 && self.region.contains(&origin) {
            self.scroll_up_from(origin, lines);
            self.carriage_return();
        }
    }

    pub(super) fn insert_blank(&mut self, count: usize) {
        self.cursor.wrap_pending = false;
        let blank = self.blank();
        let columns = self.size.columns;
        let count = count.min(columns - self.cursor.column);
        let source = self.cursor.column;
        let destination = source + count;
        let row = &mut self.grid.rows[self.cursor.line];
        for offset in (0..columns - destination).rev() {
            row.swap(destination + offset, source + offset);
        }
        row.blank(source..destination, blank);
        row.mend_wide(&[source.wrapping_sub(1), destination, columns - 1], blank);
    }

    pub(super) fn delete_chars(&mut self, count: usize) {
        self.cursor.wrap_pending = false;
        let blank = self.blank();
        let columns = self.size.columns;
        let start = self.cursor.column;
        let count = count.min(columns - start);
        let end = start + count;
        let row = &mut self.grid.rows[self.cursor.line];
        for offset in 0..columns - end {
            row.swap(start + offset, end + offset);
        }
        row.blank(columns - count..columns, blank);
        let tail = columns - count;
        row.mend_wide(&[start.wrapping_sub(1), start, tail.wrapping_sub(1), tail], blank);
    }

    pub(super) fn erase_chars(&mut self, count: usize) {
        self.cursor.wrap_pending = false;
        let blank = self.blank();
        let start = self.cursor.column;
        let end = (start + count).min(self.size.columns);
        let row = &mut self.grid.rows[self.cursor.line];
        row.blank(start..end, blank);
        row.mend_wide(&[start.wrapping_sub(1), end], blank);
    }

    /// EL: 0 right of the cursor, 1 left of it and the cursor's cell, 2 the whole line.
    pub(super) fn clear_line(&mut self, mode: u16) {
        let blank = self.blank();
        let column = self.cursor.column;
        let columns = self.size.columns;
        let range = match mode {
            0 => column..columns,
            1 => 0..column + 1,
            2 => 0..columns,
            _ => return,
        };
        self.cursor.wrap_pending = false;
        let row = &mut self.grid.rows[self.cursor.line];
        let edges = [range.start.wrapping_sub(1), range.end];
        row.blank(range, blank);
        row.mend_wide(&edges, blank);
    }

    /// ED: 0 below the cursor, 1 above it, 2 the screen, 3 the history.
    pub(super) fn clear_screen(&mut self, mode: u16) {
        let blank = self.blank();
        let (line, column) = (self.cursor.line, self.cursor.column);
        let columns = self.size.columns;
        if mode < 3 {
            self.cursor.wrap_pending = false;
        }
        match mode {
            0 => {
                self.grid.rows[line].blank(column..columns, blank);
                self.grid.rows[line].mend_wide(&[column.wrapping_sub(1)], blank);
                if line < self.size.lines - 1 {
                    self.grid.reset_rows(line + 1..self.size.lines, blank);
                }
            }
            1 => {
                self.grid.reset_rows(0..line, blank);
                let end = (column + 1).min(columns);
                self.grid.rows[line].blank(0..end, blank);
                self.grid.rows[line].mend_wide(&[end], blank);
            }
            2 => {
                if self.modes.contains(Modes::ALT_SCREEN) {
                    let lines = self.size.lines;
                    self.grid.reset_rows(0..lines, blank);
                } else {
                    let used = self
                        .grid
                        .rows
                        .iter()
                        .rposition(|row| (0..row.cells.len()).any(|column| !self.is_empty(row, column)))
                        .map_or(0, |last| last + 1);
                    // Alacritty's search for the last cell stops at the top row when there is no history, which then
                    // counts as used and goes into the history.
                    let used = if used == 0 && self.grid.history.is_empty() {
                        1
                    } else {
                        used
                    };
                    self.grid.clear_viewport(used, blank);
                }
            }
            3 => self.grid.clear_history(),
            _ => {}
        }
    }

    pub(super) fn reverse_index(&mut self) {
        self.cursor.wrap_pending = false;
        if self.cursor.line == self.region.start {
            self.scroll_down(1);
        } else {
            self.cursor.line = self.cursor.line.saturating_sub(1);
        }
    }

    pub(super) fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom.unwrap_or(self.size.lines).min(self.size.lines);
        if top >= bottom {
            return;
        }
        self.region.start = top.saturating_sub(1);
        self.region.end = bottom;
        self.goto(0, 0);
    }

    pub(super) fn decaln(&mut self) {
        for row in self.grid.rows.each_mut() {
            row.reset(StyleId(0));
            for cell in &mut row.cells {
                cell.c = 'E';
            }
            row.touch_all();
        }
        self.region = 0..self.size.lines;
        self.modes.remove(Modes::ORIGIN);
        self.cursor.style = Style::default();
        self.cursor_style_id = None;
        self.goto(0, 0);
    }

    pub(super) fn save_cursor(&mut self) {
        self.saved = self.cursor.clone();
        self.saved.origin = self.modes.contains(Modes::ORIGIN);
    }

    pub(super) fn restore_cursor(&mut self) {
        self.cursor = self.saved.clone();
        if self.cursor.origin {
            self.modes.insert(Modes::ORIGIN);
        } else {
            self.modes.remove(Modes::ORIGIN);
        }
        self.cursor_style_id = None;
    }

    /// Switches between the main and the alternate screen; entering the alternate one blanks it and saves the
    /// cursor.
    pub(super) fn swap_alt(&mut self) {
        let leaving = self.modes.contains(Modes::ALT_SCREEN);
        if !leaving {
            self.inactive_cursor = self.cursor.clone();
            self.inactive_cursor.origin = self.modes.contains(Modes::ORIGIN);
            self.saved = self.inactive_cursor.clone();
            let blank = self.blank();
            let lines = self.inactive.lines();
            self.inactive.reset_rows(0..lines, blank);
        }
        std::mem::swap(&mut self.grid, &mut self.inactive);
        std::mem::swap(&mut self.cursor, &mut self.inactive_cursor);
        std::mem::swap(&mut self.saved, &mut self.inactive_saved);
        self.cursor_style_id = None;
        self.modes.0 ^= Modes::ALT_SCREEN.0;
        if leaving {
            if self.cursor.origin {
                self.modes.insert(Modes::ORIGIN);
            } else {
                self.modes.remove(Modes::ORIGIN);
            }
        }
    }

    /// RIS: everything back to how a new terminal starts.
    pub(super) fn reset(&mut self) {
        if self.modes.contains(Modes::ALT_SCREEN) {
            std::mem::swap(&mut self.grid, &mut self.inactive);
        }
        self.active_charset = 0;
        for grid in [&mut self.grid, &mut self.inactive] {
            grid.clear_history();
            let lines = grid.lines();
            grid.reset_rows(0..lines, StyleId(0));
        }
        self.cursor = Cursor::default();
        self.saved = Cursor::default();
        self.inactive_cursor = Cursor::default();
        self.inactive_saved = Cursor::default();
        self.region = 0..self.size.lines;
        self.tabs = tab_stops(self.size.columns);
        self.title = None;
        self.title_stack.clear();
        self.modes = Modes::default();
        self.cursor_style_id = None;
        self.cursor_shape = None;
        self.colors.fill(None);
    }

    /// DECCOLM: a terminal has no 132-column font, but the region resets and the screen clears as the mode says.
    pub(super) fn deccolm(&mut self) {
        self.set_scrolling_region(1, None);
        let blank = self.blank();
        let lines = self.size.lines;
        self.grid.reset_rows(0..lines, blank);
    }
}

/// The columns `c` takes, as `unicode-width` gives it, with the common scripts answered without its tables.
#[inline(always)]
pub(crate) fn char_width(c: char) -> Option<usize> {
    match c {
        '\u{a0}'..='\u{ac}' | '\u{ae}'..='\u{2ff}' => Some(1),
        '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' => Some(2),
        _ => table_width(c),
    }
}

#[inline(never)]
fn table_width(c: char) -> Option<usize> {
    c.width()
}

#[cfg(test)]
mod width_tests {
    use unicode_width::UnicodeWidthChar;

    #[test]
    fn the_short_ways_answer_as_the_tables_do() {
        let ranges = ['\u{a0}'..='\u{2ff}', '\u{4e00}'..='\u{9fff}', '\u{ac00}'..='\u{d7a3}'];
        for c in ranges.into_iter().flatten() {
            assert_eq!(super::char_width(c), c.width(), "{:x}", c as u32);
        }
    }
}
