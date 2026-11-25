//! Resizing a grid (E4): fewer or more lines move rows between screen and history, fewer or more columns rewrap the
//! lines that wrapped, keeping the cursor on its character. The rules follow `alacritty_terminal`'s so that the
//! differential tests hold across resizes; the rows are walked from the bottom up as Alacritty's storage keeps them.

use crate::cell::{Cell, CellFlags, Row};
use crate::grid::Grid;
use crate::style::StyleId;

/// A cursor's place, as resizing moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// The line on the screen.
    pub line: usize,
    /// The column; past the last one while a wrap is pending, during the resize.
    pub column: usize,
    /// The next character wraps first.
    pub wrap_pending: bool,
}

/// Whether a cell holds nothing worth keeping, as `Terminal` decides with its styles.
pub type Empty<'a> = &'a dyn Fn(&Row, usize) -> bool;

impl Grid {
    /// Resizes to `lines` × `columns`, rewrapping when `reflow`; `cursor` and `saved` follow their characters.
    pub fn resize(
        &mut self,
        reflow: bool,
        lines: usize,
        columns: usize,
        cursor: &mut Place,
        saved: &mut Place,
        empty: Empty<'_>,
    ) {
        match self.lines().cmp(&lines) {
            std::cmp::Ordering::Less => self.grow_lines(lines, cursor, saved),
            std::cmp::Ordering::Greater => self.shrink_lines(lines, cursor, saved),
            std::cmp::Ordering::Equal => {}
        }
        match self.columns().cmp(&columns) {
            std::cmp::Ordering::Less => self.grow_columns(reflow, columns, cursor, empty),
            std::cmp::Ordering::Greater => self.shrink_columns(reflow, columns, cursor, saved, empty),
            std::cmp::Ordering::Equal => {}
        }
        self.rows.each_mut().for_each(Row::touch_all);
    }

    fn grow_lines(&mut self, target: usize, cursor: &mut Place, saved: &mut Place) {
        let added = target - self.lines();
        let from_history = self.history.len().min(added);
        for _ in 0..from_history {
            if let Some(row) = self.history.pop_back() {
                self.rows.insert(0, row);
            }
        }
        let columns = self.columns();
        for _ in from_history..added {
            let mut row = Row::new(columns, StyleId(0));
            row.serial = self.next_serial();
            self.rows.push_back(row);
        }
        cursor.line += from_history;
        saved.line += from_history;
    }

    fn shrink_lines(&mut self, target: usize, cursor: &mut Place, saved: &mut Place) {
        let required = (cursor.line + 1).saturating_sub(target);
        if required > 0 {
            let lines = self.lines();
            self.scroll_up(0..lines, required, StyleId(0));
            cursor.line = cursor.line.min(target - 1);
        }
        saved.line = saved.line.min(target - 1);
        self.rows.truncate(target);
    }

    /// All rows, bottom first, as Alacritty's storage holds them; the grid keeps none until `put_raw`.
    fn take_raw(&mut self) -> Vec<Row> {
        let mut raw: Vec<Row> = self.rows.take_all().into_iter().rev().collect();
        raw.extend(self.history.take_all().into_iter().rev());
        raw
    }

    /// Puts `raw`, bottom first, back as screen and history of `lines` rows.
    fn put_raw(&mut self, mut raw: Vec<Row>, lines: usize) {
        raw.reverse();
        for row in &mut raw {
            if row.serial == 0 {
                row.serial = self.next_serial();
            }
            row.touch_all();
        }
        let history = raw.len().saturating_sub(lines);
        self.history = raw.drain(..history).collect();
        self.rows = raw.into();
        while self.history.len() > self.max_history {
            self.history.pop_front();
            self.dropped += 1;
        }
        self.display_offset = self.display_offset.min(self.history.len());
    }

    fn grow_columns(&mut self, reflow: bool, columns: usize, cursor: &mut Place, empty: Empty<'_>) {
        let lines = self.lines();
        self.set_columns(columns);
        let wraps = |row: &Row| {
            let len = row.cells.len();
            reflow && len > 0 && len < columns && row.cells[len - 1].flags.contains(CellFlags::WRAPPED)
        };
        let clear = |row: &Row| (0..row.cells.len()).all(|column| empty(row, column));
        if cursor.wrap_pending && reflow {
            cursor.wrap_pending = false;
            cursor.column += 1;
        }
        let raw = self.take_raw();
        let mut reversed: Vec<Row> = Vec::with_capacity(raw.len());
        let mut cursor_line_delta = 0usize;
        for (i, mut row) in raw.into_iter().enumerate().rev() {
            let Some(last) = reversed.last_mut().filter(|last| wraps(last)) else {
                reversed.push(row);
                continue;
            };
            if let Some(cell) = last.cells.last_mut() {
                cell.flags.remove(CellFlags::WRAPPED);
            }
            let mut last_len = last.cells.len();
            if last_len >= 1 && last.cells[last_len - 1].flags.contains(CellFlags::LEADING_WIDE_SPACER) {
                last.cells.truncate(last_len - 1);
                last_len -= 1;
            }
            let mut wrapped = columns - last_len;
            let len = row.cells.len().min(wrapped);
            let mut moved = if row.cells[len - 1].flags.contains(CellFlags::WIDE) {
                wrapped -= 1;
                let mut moved = front_split_off(&mut row, len - 1);
                let mut spacer = Cell::default();
                spacer.flags.insert(CellFlags::LEADING_WIDE_SPACER);
                moved.cells.push(spacer);
                moved
            } else {
                front_split_off(&mut row, len)
            };
            append(last, &mut moved);
            let cursor_buffer_line = lines - cursor.line - 1;
            if i == cursor_buffer_line && reflow {
                let line = isize::try_from(cursor.line).unwrap_or(0);
                let mut target = sub_place(line, cursor.column, wrapped, columns, lines);
                if target.1 == 0 && clear(&row) {
                    cursor.wrap_pending = true;
                    target = sub_place(target.0, target.1, 1, columns, lines);
                }
                cursor.column = target.1;
                let delta = isize::try_from(cursor.line).unwrap_or(0) - target.0;
                if delta != 0 && clear(&row) {
                    continue;
                }
                cursor_line_delta += usize::try_from(delta).unwrap_or(0);
            } else if clear(&row) {
                if i < cursor_buffer_line {
                    cursor.line += 1;
                }
                continue;
            }
            if let Some(cell) = reversed.last_mut().and_then(|last| last.cells.last_mut()) {
                cell.flags.insert(CellFlags::WRAPPED);
            }
            reversed.push(row);
        }
        if reversed.len() < lines {
            let delta = lines - reversed.len();
            cursor.line = cursor.line.saturating_sub(delta);
            reversed.resize_with(lines, || Row::new(columns, StyleId(0)));
        }
        if cursor_line_delta != 0 {
            let cursor_buffer_line = lines - cursor.line - 1;
            let available = cursor_buffer_line.min(reversed.len() - lines);
            let overflow = cursor_line_delta.saturating_sub(available);
            reversed.truncate(reversed.len() + overflow - cursor_line_delta);
            cursor.line = cursor.line.saturating_sub(overflow);
        }
        let mut raw: Vec<Row> = reversed.into_iter().rev().collect();
        for row in &mut raw {
            if row.cells.len() < columns {
                row.cells.resize(columns, Cell::default());
            }
        }
        self.put_raw(raw, lines);
    }

    fn shrink_columns(
        &mut self,
        reflow: bool,
        columns: usize,
        cursor: &mut Place,
        saved: &mut Place,
        empty: Empty<'_>,
    ) {
        let lines = self.lines();
        self.set_columns(columns);
        if cursor.wrap_pending && reflow {
            cursor.wrap_pending = false;
            cursor.column += 1;
        }
        let raw = self.take_raw();
        let mut new_raw: Vec<Row> = Vec::with_capacity(raw.len());
        let mut buffered: Option<Row> = None;
        for (i, mut row) in raw.into_iter().enumerate().rev() {
            if let Some(mut front) = buffered.take() {
                if i == lines.wrapping_sub(cursor.line + 1) {
                    cursor.column += front.cells.len();
                }
                append(&mut front, &mut row);
                row = front;
            }
            loop {
                let cursor_buffer_line = lines - cursor.line - 1;
                let mut wrapped = match shrink(&mut row, columns, empty) {
                    Some(wrapped) if reflow => wrapped,
                    _ => {
                        if reflow && i == cursor_buffer_line && cursor.column > columns {
                            Row::from_parts(Vec::new(), Vec::new(), 0)
                        } else {
                            new_raw.push(row);
                            break;
                        }
                    }
                };
                if row.cells.len() >= columns && row.cells[columns - 1].flags.contains(CellFlags::WIDE) {
                    let mut spacer = Cell::default();
                    spacer.flags.insert(CellFlags::LEADING_WIDE_SPACER);
                    let wide = std::mem::replace(&mut row.cells[columns - 1], spacer);
                    let extra = take_extra(&mut row, columns - 1);
                    wrapped.cells.insert(0, wide);
                    for (column, _) in &mut wrapped.extras {
                        *column += 1;
                    }
                    if let Some(extra) = extra {
                        wrapped.extras.push((0, extra));
                    }
                }
                let len = wrapped.cells.len();
                if len > 0 && wrapped.cells[len - 1].flags.contains(CellFlags::LEADING_WIDE_SPACER) {
                    if len == 1 {
                        row.cells[columns - 1].flags.insert(CellFlags::WRAPPED);
                        new_raw.push(row);
                        break;
                    }
                    wrapped.cells[len - 2].flags.insert(CellFlags::WRAPPED);
                    wrapped.cells.truncate(len - 1);
                }
                new_raw.push(row);
                if let Some(cell) = new_raw.last_mut().and_then(|r| r.cells.last_mut()) {
                    cell.flags.insert(CellFlags::WRAPPED);
                }
                let continues = wrapped
                    .cells
                    .last()
                    .is_some_and(|c| c.flags.contains(CellFlags::WRAPPED) && i >= 1);
                if continues && wrapped.cells.len() < columns {
                    if let Some(cell) = wrapped.cells.last_mut() {
                        cell.flags.remove(CellFlags::WRAPPED);
                    }
                    buffered = Some(wrapped);
                    break;
                }
                if (i == cursor_buffer_line && cursor.column < columns) || i < cursor_buffer_line {
                    cursor.line = cursor.line.saturating_sub(1);
                }
                if i == cursor_buffer_line && cursor.column >= columns {
                    cursor.column -= columns;
                }
                if wrapped.cells.len() < columns {
                    wrapped.cells.resize(columns, Cell::default());
                }
                row = wrapped;
            }
        }
        let mut raw: Vec<Row> = new_raw.into_iter().rev().collect();
        raw.truncate(self.max_history + lines);
        self.put_raw(raw, lines);
        self.settle_cursor(reflow, columns, lines, cursor);
        saved.column = saved.column.min(columns - 1);
    }
}

impl Grid {
    /// Where the cursor ends after the columns shrank: on the screen, at the end of a line that does not wrap with
    /// its wrap pending.
    fn settle_cursor(&self, reflow: bool, columns: usize, lines: usize, cursor: &mut Place) {
        if !reflow {
            cursor.column = cursor.column.min(columns - 1);
        } else if cursor.column == columns
            && !self.rows[cursor.line].cells[columns - 1]
                .flags
                .contains(CellFlags::WRAPPED)
        {
            cursor.wrap_pending = true;
            cursor.column -= 1;
        } else {
            cursor.line = cursor.line.min(lines - 1);
            cursor.column = cursor.column.min(columns - 1);
        }
    }
}

/// `count` cells before (`line`, `column`), wrapping to the lines above and clamped to the screen as Alacritty's
/// `Point::sub` with the cursor's boundary does: above the screen is its first cell, below it its last.
fn sub_place(line: isize, column: usize, count: usize, columns: usize, lines: usize) -> (isize, usize) {
    let changes = (count + columns - 1).saturating_sub(column) / columns;
    let line = line - isize::try_from(changes).unwrap_or(0);
    let column = ((columns + column - count % columns) % columns).min(columns - 1);
    let bottom = isize::try_from(lines).unwrap_or(1) - 1;
    if line < 0 {
        (0, 0)
    } else if line > bottom {
        (bottom, columns - 1)
    } else {
        (line, column)
    }
}

/// The first `at` cells of `row`, taken out of it with their extras.
fn front_split_off(row: &mut Row, at: usize) -> Row {
    let rest = row.cells.split_off(at);
    let front = std::mem::replace(&mut row.cells, rest);
    let mut front_extras = Vec::new();
    row.extras.retain(|(column, extra)| {
        if usize::from(*column) < at {
            front_extras.push((*column, extra.clone()));
            false
        } else {
            true
        }
    });
    for (column, _) in &mut row.extras {
        *column -= u16::try_from(at).unwrap_or(0);
    }
    Row::from_parts(front, front_extras, std::mem::take(&mut row.serial))
}

/// Appends `tail`'s cells and extras to `row`.
fn append(row: &mut Row, tail: &mut Row) {
    let offset = u16::try_from(row.cells.len()).unwrap_or(u16::MAX);
    row.cells.append(&mut tail.cells);
    row.extras
        .extend(tail.extras.drain(..).map(|(column, extra)| (column + offset, extra)));
}

/// Cuts `row` to `columns`; the cells beyond, up to the last one not empty, if any.
fn shrink(row: &mut Row, columns: usize, empty: Empty<'_>) -> Option<Row> {
    if row.cells.len() <= columns {
        return None;
    }
    let keep = (columns..row.cells.len())
        .rposition(|column| !empty(row, column))
        .map_or(0, |index| index + 1);
    let mut tail = front_split_off(row, columns);
    std::mem::swap(&mut tail, row);
    tail.cells.truncate(keep);
    tail.extras.retain(|(column, _)| usize::from(*column) < keep);
    (!tail.cells.is_empty()).then_some(tail)
}

fn take_extra(row: &mut Row, column: usize) -> Option<crate::cell::Extra> {
    let index = row.extras.iter().position(|(at, _)| usize::from(*at) == column)?;
    Some(row.extras.remove(index).1)
}
