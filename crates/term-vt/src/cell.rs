//! A cell, eight bytes: its character, its style's number and its flags (E4). What few cells carry besides — joining
//! marks and emoji modifiers, links — lives in a table of the row.

use std::sync::Arc;

use crate::style::StyleId;

/// What a cell is besides its text and style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags(pub u16);

impl CellFlags {
    /// A double-width character, drawn across this cell and the next.
    pub const WIDE: CellFlags = CellFlags(1);
    /// The second half of a double-width character.
    pub const WIDE_SPACER: CellFlags = CellFlags(1 << 1);
    /// A blank at the end of a line, left where a double-width character did not fit.
    pub const LEADING_WIDE_SPACER: CellFlags = CellFlags(1 << 2);
    /// The line continues on the next row, wrapped after this cell.
    pub const WRAPPED: CellFlags = CellFlags(1 << 3);
    /// The cell has an entry in its row's table of extras.
    pub const EXTRA: CellFlags = CellFlags(1 << 4);
}

impl CellFlags {
    /// Whether any of `other` is set.
    pub fn intersects(self, other: CellFlags) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether all of `other` are set.
    pub fn contains(self, other: CellFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets `other`.
    pub fn insert(&mut self, other: CellFlags) {
        self.0 |= other.0;
    }

    /// Clears `other`.
    pub fn remove(&mut self, other: CellFlags) {
        self.0 &= !other.0;
    }
}

/// One cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The character, a space where nothing was written.
    pub c: char,
    /// The style's number.
    pub style: StyleId,
    /// Width and wrapping.
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell {
            c: ' ',
            style: StyleId(0),
            flags: CellFlags::default(),
        }
    }
}

impl Cell {
    /// A blank cell in style `style`.
    pub fn blank(style: StyleId) -> Cell {
        Cell {
            style,
            ..Cell::default()
        }
    }
}

/// What a few cells carry besides their character.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extra {
    /// Characters of no width drawn over the cell's: combining marks, variation selectors, joiners.
    pub zero_width: String,
    /// The link the program put over the cell (OSC 8).
    pub link: Option<Arc<Link>>,
}

/// A link of OSC 8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The program's id for the link, which joins cells of one link that wrapped.
    pub id: Option<String>,
    /// The address.
    pub uri: String,
}

/// A row of cells and the extras of some of them.
#[derive(Debug, Clone)]
pub struct Row {
    /// The cells, as many as the grid has columns.
    pub cells: Vec<Cell>,
    /// The extras, by column; kept short, so a search through it is cheap.
    pub extras: Vec<(u16, Extra)>,
    /// The row's number for anchors such as prompt marks (E4): new for each line that is written anew, kept when
    /// the row scrolls or a wrapped line is rewrapped, so a mark stays with its line; 0 until the grid numbers it.
    pub serial: u64,
    /// The cells from here on are blank in `tail`, so blanking the row again in that style writes only those before
    /// (scrolling a mostly empty line in costs what it holds, not its width). Writers outside this type call `touch`.
    used: usize,
    tail: StyleId,
}

impl PartialEq for Row {
    fn eq(&self, other: &Row) -> bool {
        self.cells == other.cells && self.extras == other.extras && self.serial == other.serial
    }
}

impl Eq for Row {}

impl Row {
    /// A row of `columns` blank cells in style `style`.
    pub fn new(columns: usize, style: StyleId) -> Row {
        Row {
            cells: vec![Cell::blank(style); columns],
            extras: Vec::new(),
            serial: 0,
            used: 0,
            tail: style,
        }
    }

    /// A row of these cells, extras and serial.
    pub fn from_parts(cells: Vec<Cell>, extras: Vec<(u16, Extra)>, serial: u64) -> Row {
        let used = cells.len();
        Row {
            cells,
            extras,
            serial,
            used,
            tail: StyleId(0),
        }
    }

    /// A row as the history stored it: cells from `used` on blank in `tail`.
    pub(crate) fn from_stored(
        cells: Vec<Cell>,
        extras: Vec<(u16, Extra)>,
        serial: u64,
        used: usize,
        tail: StyleId,
    ) -> Row {
        Row {
            cells,
            extras,
            serial,
            used,
            tail,
        }
    }

    /// How far the row may be written, and the style the cells after that are blank in.
    pub(crate) fn stored(&self) -> (usize, StyleId) {
        (self.used, self.tail)
    }

    /// Notes that cells before `end` may have been written through `cells`.
    pub fn touch(&mut self, end: usize) {
        self.used = self.used.max(end.min(self.cells.len()));
    }

    /// Notes that any cell may have been written through `cells`.
    pub fn touch_all(&mut self) {
        self.used = self.cells.len();
    }

    /// Renumbers the cells' styles, `map` giving each old number its new one.
    pub fn renumber_styles(&mut self, map: &[StyleId]) {
        for cell in &mut self.cells {
            cell.style = map[usize::from(cell.style.0)];
        }
        self.tail = map.get(usize::from(self.tail.0)).copied().unwrap_or_default();
    }

    /// Blanks every cell in style `style`.
    pub fn reset(&mut self, style: StyleId) {
        debug_assert!(
            self.cells[self.used.min(self.cells.len())..]
                .iter()
                .all(|cell| *cell == Cell::blank(self.tail)),
            "a cell past `used` was written without `touch`"
        );
        let end = if style == self.tail {
            self.used.min(self.cells.len())
        } else {
            self.cells.len()
        };
        self.cells[..end].fill(Cell::blank(style));
        self.extras.clear();
        self.used = 0;
        self.tail = style;
    }

    /// Blanks the cells of `range` in style `style`.
    pub fn blank(&mut self, range: std::ops::Range<usize>, style: StyleId) {
        let end = range.end.min(self.cells.len());
        let start = range.start.min(end);
        for cell in &mut self.cells[start..end] {
            *cell = Cell::blank(style);
        }
        self.extras
            .retain(|(column, _)| !(start..end).contains(&usize::from(*column)));
        if style != self.tail {
            self.touch(end);
        } else if end == self.cells.len() {
            self.used = self.used.min(start);
        }
    }

    /// The extras of cell `column`.
    pub fn extra(&self, column: usize) -> Option<&Extra> {
        self.extras
            .iter()
            .find(|(at, _)| usize::from(*at) == column)
            .map(|(_, extra)| extra)
    }

    /// The extras of cell `column`, made if it has none.
    pub fn extra_mut(&mut self, column: usize) -> &mut Extra {
        let at = u16::try_from(column).unwrap_or(u16::MAX);
        if let Some(index) = self.extras.iter().position(|(c, _)| *c == at) {
            return &mut self.extras[index].1;
        }
        if let Some(cell) = self.cells.get_mut(column) {
            cell.flags.insert(CellFlags::EXTRA);
        }
        self.touch(column + 1);
        self.extras.push((at, Extra::default()));
        let last = self.extras.len() - 1;
        &mut self.extras[last].1
    }

    /// Forgets the extras of cell `column`.
    pub fn clear_extra(&mut self, column: usize) {
        self.extras.retain(|(at, _)| usize::from(*at) != column);
        if let Some(cell) = self.cells.get_mut(column) {
            cell.flags.remove(CellFlags::EXTRA);
        }
        self.touch(column + 1);
    }

    /// Swaps cells `a` and `b` with their extras.
    pub fn swap(&mut self, a: usize, b: usize) {
        self.cells.swap(a, b);
        self.touch(a.max(b) + 1);
        for (column, _) in &mut self.extras {
            if usize::from(*column) == a {
                *column = u16::try_from(b).unwrap_or(u16::MAX);
            } else if usize::from(*column) == b {
                *column = u16::try_from(a).unwrap_or(u16::MAX);
            }
        }
    }

    /// Blanks in style `style` each half of a wide character at the columns `at` whose other half an edit took
    /// away, as xterm does; an edit splits one only at the edges of what it blanked or moved.
    pub fn mend_wide(&mut self, at: &[usize], style: StyleId) {
        for &column in at {
            let Some(cell) = self.cells.get(column) else { continue };
            let orphan = if cell.flags.contains(CellFlags::WIDE) {
                self.cells
                    .get(column + 1)
                    .is_none_or(|next| !next.flags.contains(CellFlags::WIDE_SPACER))
            } else {
                cell.flags.contains(CellFlags::WIDE_SPACER)
                    && (column == 0 || !self.cells[column - 1].flags.contains(CellFlags::WIDE))
            };
            if orphan {
                self.blank(column..column + 1, style);
            }
        }
    }

    /// Whether the row continues on the next one.
    pub fn wrapped(&self) -> bool {
        self.cells
            .last()
            .is_some_and(|cell| cell.flags.contains(CellFlags::WRAPPED))
    }

    /// The row's text, double-width characters once, zero-width ones after their cell's.
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.cells.len());
        for (column, cell) in self.cells.iter().enumerate() {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            text.push(cell.c);
            if cell.flags.contains(CellFlags::EXTRA)
                && let Some(extra) = self.extra(column)
            {
                text.push_str(&extra.zero_width);
            }
        }
        text
    }
}
