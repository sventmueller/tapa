//! The rows above the screen (E4, F8), oldest first, in pages as Ghostty keeps its scrollback: a row that leaves the
//! screen is copied to the end of the newest page, its cells up to the last one that is not blank in the row's
//! blank style, and the screen keeps its buffer for the row that comes in. A full history drops its oldest page
//! whole and keeps the memory for the next one. While the session is idle, older pages are packed: their cells
//! become runs of one style and flags with the characters as UTF-8. Reading a row builds a copy.

use std::borrow::Cow;
use std::collections::VecDeque;

use crate::cell::{Cell, CellFlags, Extra, Row};
use crate::style::StyleId;

/// The most cells a page holds before the next one starts.
const PAGE_CELLS: usize = 1 << 15;

/// The most rows a page holds before the next one starts.
const PAGE_ROWS: usize = 1 << 10;

/// The rows above the screen.
#[derive(Debug, Clone, Default)]
pub struct History {
    pages: VecDeque<Page>,
    len: usize,
    /// The cell memory of the page dropped last, for the next page.
    spare: Vec<Cell>,
}

#[derive(Debug, Clone, Default)]
struct Page {
    /// The open rows' cells one after the other; empty once packed.
    cells: Vec<Cell>,
    /// The packed rows' runs one after the other.
    runs: Vec<u8>,
    lines: VecDeque<Line>,
    packed: bool,
}

/// A row of a page: where its cells or runs are, and what fills it up to its width.
#[derive(Debug, Clone)]
struct Line {
    start: usize,
    end: usize,
    columns: u16,
    tail: StyleId,
    serial: u64,
    extras: Box<[(u16, Extra)]>,
}

impl History {
    /// The number of rows.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Forgets all rows.
    pub fn clear(&mut self) {
        self.pages.clear();
        self.len = 0;
    }

    /// Adds a copy of `row` as the newest.
    pub fn push_back(&mut self, row: &Row) {
        let (used, tail) = row.stored();
        let blank = Cell::blank(tail);
        let kept = row.cells[..used.min(row.cells.len())]
            .iter()
            .rposition(|cell| *cell != blank)
            .map_or(0, |last| last + 1);
        let fits = self.pages.back().is_some_and(|page| {
            !page.packed && page.lines.len() < PAGE_ROWS && page.cells.len() + kept <= PAGE_CELLS.max(kept)
        });
        if !fits {
            let mut cells = std::mem::take(&mut self.spare);
            cells.clear();
            if cells.capacity() == 0 {
                cells.reserve(PAGE_CELLS.max(kept));
            }
            self.pages.push_back(Page {
                cells,
                ..Page::default()
            });
        }
        let Some(page) = self.pages.back_mut() else { return };
        let start = page.cells.len();
        page.cells.extend_from_slice(&row.cells[..kept]);
        page.lines.push_back(Line {
            start,
            end: start + kept,
            columns: u16::try_from(row.cells.len()).unwrap_or(u16::MAX),
            tail,
            serial: row.serial,
            extras: row.extras.clone().into_boxed_slice(),
        });
        self.len += 1;
    }

    /// Forgets the oldest row; a page left empty goes, its memory kept for the next.
    pub fn drop_front(&mut self) {
        let Some(page) = self.pages.front_mut() else { return };
        if page.lines.pop_front().is_some() {
            self.len -= 1;
        }
        if page.lines.is_empty()
            && let Some(page) = self.pages.pop_front()
            && page.cells.capacity() >= self.spare.capacity()
        {
            self.spare = page.cells;
        }
    }

    /// Takes the oldest row.
    pub fn pop_front(&mut self) -> Option<Row> {
        let row = self.get(0)?.into_owned();
        self.drop_front();
        Some(row)
    }

    /// Takes the newest row.
    pub fn pop_back(&mut self) -> Option<Row> {
        let page = self.pages.back_mut()?;
        let line = page.lines.pop_back()?;
        let row = page.row(&line);
        if page.packed {
            page.runs.truncate(line.start);
        } else {
            page.cells.truncate(line.start);
        }
        if page.lines.is_empty() {
            self.pages.pop_back();
        }
        self.len -= 1;
        Some(row)
    }

    /// Row `index` from the oldest, as a copy.
    pub fn get(&self, index: usize) -> Option<Cow<'_, Row>> {
        let mut index = index;
        for page in &self.pages {
            match page.lines.get(index) {
                Some(line) => return Some(Cow::Owned(page.row(line))),
                None => index -= page.lines.len(),
            }
        }
        None
    }

    /// Clears `flag` on the newest row's last cell, as writing over the start of the screen's top row does for the
    /// spacer a wide character left there when it wrapped.
    pub fn clear_last_flag(&mut self, flag: CellFlags) {
        let Some(page) = self.pages.back_mut() else { return };
        let Some(line) = page.lines.back() else { return };
        if line.end - line.start < usize::from(line.columns) {
            return;
        }
        if page.packed {
            page.unpack();
        }
        if let Some(line) = page.lines.back()
            && let Some(cell) = page.cells.get_mut(line.end.wrapping_sub(1))
        {
            cell.flags.remove(flag);
        }
    }

    /// The rows, oldest first, as copies.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = Cow<'_, Row>> {
        self.pages
            .iter()
            .flat_map(|page| page.lines.iter().map(|line| Cow::Owned(page.row(line))))
    }

    /// Renumbers the styles of all rows, `map` giving each old number its new one.
    pub fn renumber_styles(&mut self, map: &[StyleId]) {
        for page in &mut self.pages {
            if page.packed {
                page.unpack();
            }
            for cell in &mut page.cells {
                cell.style = map[usize::from(cell.style.0)];
            }
            for line in &mut page.lines {
                line.tail = map.get(usize::from(line.tail.0)).copied().unwrap_or_default();
            }
        }
    }

    /// The serials of the rows, oldest first.
    pub fn serials(&self) -> impl Iterator<Item = u64> + '_ {
        self.pages
            .iter()
            .flat_map(|page| page.lines.iter().map(|line| line.serial))
    }

    /// Takes all rows, oldest first.
    pub fn take_all(&mut self) -> Vec<Row> {
        let rows = self.iter().map(Cow::into_owned).collect();
        self.clear();
        rows
    }

    /// Packs the open pages whose rows are all older than the newest `keep`, oldest first, until at least `most`
    /// rows are packed; how many it packed, so that a caller can do the work in short steps and stop at 0.
    pub fn pack(&mut self, keep: usize, most: usize) -> usize {
        let end = self.len.saturating_sub(keep);
        let mut before = 0;
        let mut packed = 0;
        for page in &mut self.pages {
            before += page.lines.len();
            if before > end || packed >= most {
                break;
            }
            if !page.packed {
                page.pack();
                packed += page.lines.len();
            }
        }
        packed
    }

    /// The bytes the rows take, roughly: their cells and runs and the record of each row, not what the allocator
    /// adds.
    pub fn bytes(&self) -> usize {
        self.pages
            .iter()
            .map(|page| {
                page.cells.capacity() * std::mem::size_of::<Cell>()
                    + page.runs.capacity()
                    + page.lines.len() * std::mem::size_of::<Line>()
            })
            .sum()
    }
}

impl FromIterator<Row> for History {
    fn from_iter<I: IntoIterator<Item = Row>>(rows: I) -> History {
        let mut history = History::default();
        for row in rows {
            history.push_back(&row);
        }
        history
    }
}

impl Page {
    fn row(&self, line: &Line) -> Row {
        let columns = usize::from(line.columns);
        let mut cells = Vec::with_capacity(columns);
        if self.packed {
            decode(&self.runs[line.start..line.end], &mut cells);
        } else {
            cells.extend_from_slice(&self.cells[line.start..line.end]);
        }
        let used = cells.len();
        cells.resize(columns, Cell::blank(line.tail));
        Row::from_stored(cells, line.extras.to_vec(), line.serial, used, line.tail)
    }

    fn pack(&mut self) {
        let mut runs = Vec::with_capacity(self.cells.len() / 2);
        for line in &mut self.lines {
            let start = runs.len();
            encode(&self.cells[line.start..line.end], &mut runs);
            line.start = start;
            line.end = runs.len();
        }
        runs.shrink_to_fit();
        self.runs = runs;
        self.cells = Vec::new();
        self.packed = true;
    }

    fn unpack(&mut self) {
        let mut cells = Vec::new();
        for line in &mut self.lines {
            let start = cells.len();
            decode(&self.runs[line.start..line.end], &mut cells);
            line.start = start;
            line.end = cells.len();
        }
        self.cells = cells;
        self.runs = Vec::new();
        self.packed = false;
    }
}

/// `cells` as runs of one style and flags: count, style and flags as varints, then the characters as UTF-8.
fn encode(cells: &[Cell], out: &mut Vec<u8>) {
    let mut cells = cells.iter().peekable();
    while let Some(first) = cells.next() {
        let mut run = vec![first.c];
        while let Some(next) = cells.next_if(|next| next.style == first.style && next.flags == first.flags) {
            run.push(next.c);
        }
        put(out, run.len() as u64);
        put(out, u64::from(first.style.0));
        put(out, u64::from(first.flags.0));
        for c in run {
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
        }
    }
}

fn decode(runs: &[u8], cells: &mut Vec<Cell>) {
    let mut at = 0;
    while at < runs.len() {
        let count = take(runs, &mut at);
        let style = StyleId(u16::try_from(take(runs, &mut at)).unwrap_or(0));
        let flags = CellFlags(u16::try_from(take(runs, &mut at)).unwrap_or(0));
        for _ in 0..count {
            let Some(c) = character(runs, &mut at) else { break };
            cells.push(Cell { c, style, flags });
        }
    }
}

fn character(bytes: &[u8], at: &mut usize) -> Option<char> {
    let length = match bytes.get(*at)? {
        0..0x80 => 1,
        0xc0..0xe0 => 2,
        0xe0..0xf0 => 3,
        _ => 4,
    };
    let c = std::str::from_utf8(bytes.get(*at..*at + length)?)
        .ok()?
        .chars()
        .next()?;
    *at += length;
    Some(c)
}

fn put(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value.to_le_bytes()[0] | 0x80);
        value >>= 7;
    }
    out.push(value.to_le_bytes()[0]);
}

fn take(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value = 0;
    let mut shift = 0;
    while let Some(&byte) = bytes.get(*at) {
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            break;
        }
        shift += 7;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, columns: usize, style: u16) -> Row {
        let mut row = Row::new(columns, StyleId(0));
        for (cell, c) in row.cells.iter_mut().zip(text.chars()) {
            cell.c = c;
            cell.style = StyleId(style);
        }
        row.touch_all();
        row
    }

    #[test]
    fn a_stored_row_reads_back_the_same_open_and_packed() {
        let mut wide = row("ab€😀 x", 200, 3);
        wide.cells[2].flags = CellFlags::WIDE;
        wide.cells[3].flags = CellFlags::WIDE_SPACER;
        wide.cells[199] = Cell {
            c: 'z',
            style: StyleId(700),
            flags: CellFlags::WRAPPED,
        };
        wide.extra_mut(1).zero_width.push('\u{301}');
        wide.serial = 42;
        let mut coloured = Row::new(30, StyleId(5));
        coloured.cells[0].c = 'q';
        coloured.touch(1);
        for original in [wide, row("", 80, 0), row("    ", 4, 9), coloured] {
            let mut history: History = [original.clone()].into_iter().collect();
            assert_eq!(*history.get(0).unwrap(), original);
            assert_eq!(history.pack(0, usize::MAX), 1);
            assert_eq!(*history.get(0).unwrap(), original);
            assert_eq!(history.serials().next(), Some(original.serial));
            assert_eq!(history.pop_front().unwrap(), original);
            assert!(history.is_empty());
        }
    }

    #[test]
    fn rows_fill_pages_and_a_full_history_drops_whole_pages_and_reuses_their_memory() {
        let mut history = History::default();
        for n in 0..3000 {
            history.push_back(&row(&format!("line {n}"), 200, 1));
        }
        assert_eq!(history.len(), 3000);
        assert_eq!(history.pages.len(), 3);
        for _ in 0..1024 {
            history.drop_front();
        }
        assert_eq!(history.pages.len(), 2);
        assert!(history.spare.capacity() > 0);
        assert_eq!(history.get(0).unwrap().text().trim_end(), "line 1024");
        assert_eq!(history.pop_back().unwrap().text().trim_end(), "line 2999");
        assert_eq!(history.len(), 1975);
    }

    #[test]
    fn packing_keeps_the_newest_rows_works_in_steps_and_saves_most_of_the_memory() {
        let mut history: History = (0..3000)
            .map(|n| row(&format!("line {n}: some output"), 200, 1))
            .collect();
        let open = history.bytes();
        assert_eq!(history.pack(900, 1), 1024);
        assert_eq!(history.pack(900, 1), 1024);
        assert_eq!(history.pack(900, 1), 0, "the newest page holds rows of the newest 900");
        assert!(history.pages[1].packed && !history.pages[2].packed);
        let packed = history.bytes();
        assert!(packed * 2 < open, "{packed} of {open} bytes");
        assert_eq!(history.get(10).unwrap().text().trim_end(), "line 10: some output");
        history.renumber_styles(&[StyleId(0), StyleId(7)]);
        assert_eq!(history.get(10).unwrap().cells[0].style, StyleId(7));
    }
}
