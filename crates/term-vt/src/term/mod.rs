//! The terminal of the VT core (E4): what the parsed output does to the screen, the alternate screen, the cursor
//! and its saved copy, the scrolling region, tab stops, modes and styles; and what it reports back to the program
//! and to its owner. Its behaviour follows `alacritty_terminal`, which the reference tests hold it to (E11).

mod blocks;
pub mod colors;
mod marks;
mod modes;
mod ops;
mod perform;
mod reports;
mod sgr;

use std::borrow::Cow;
use std::sync::Arc;

pub use blocks::{Block, BlockState};
pub use marks::{Mark, MarkKind, TypedLine};
pub use modes::Modes;

use crate::cell::{Link, Row};
use crate::color::Color;
use crate::grid::Grid;
use crate::parser::Parser;
use crate::style::{Style, StyleId, Styles};

/// How many of the newest history rows stay unpacked, as those are the ones scrolled back to most.
const OPEN_ROWS: usize = 500;

/// A style table this full after compacting freed too little to compact again at once.
const STYLES_CROWDED: usize = 7 * (1 << 16) / 8;

/// The size of a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Columns.
    pub columns: usize,
    /// Screen lines.
    pub lines: usize,
}

/// A character set of the four a terminal holds (G0 to G3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Charset {
    /// ASCII as it is.
    #[default]
    Ascii,
    /// DEC's special characters and line drawing, as `ESC ( 0` selects.
    LineDrawing,
}

impl Charset {
    fn map(self, c: char) -> char {
        if self == Charset::Ascii {
            return c;
        }
        match c {
            '_' => ' ',
            '`' => '◆',
            'a' => '▒',
            'b' => '\u{2409}',
            'c' => '\u{240c}',
            'd' => '\u{240d}',
            'e' => '\u{240a}',
            'f' => '°',
            'g' => '±',
            'h' => '\u{2424}',
            'i' => '\u{240b}',
            'j' => '┘',
            'k' => '┐',
            'l' => '┌',
            'm' => '└',
            'n' => '┼',
            'o' => '⎺',
            'p' => '⎻',
            'q' => '─',
            'r' => '⎼',
            's' => '⎽',
            't' => '├',
            'u' => '┤',
            'v' => '┴',
            'w' => '┬',
            'x' => '│',
            'y' => '≤',
            'z' => '≥',
            '{' => 'π',
            '|' => '≠',
            '}' => '£',
            '~' => '·',
            _ => c,
        }
    }
}

/// Where the next character goes and how it looks.
#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    /// The line on the screen.
    pub line: usize,
    /// The column.
    pub column: usize,
    /// The style new characters get.
    pub style: Style,
    /// The link new characters get (OSC 8).
    pub link: Option<Arc<Link>>,
    /// The cursor sits past the last column; the next character wraps first.
    pub wrap_pending: bool,
    /// G0 to G3.
    pub charsets: [Charset; 4],
    /// Origin mode, as DECSC saves it with the cursor and DECRC brings it back.
    pub origin: bool,
}

impl Default for Cursor {
    fn default() -> Cursor {
        Cursor {
            line: 0,
            column: 0,
            style: Style::default(),
            link: None,
            wrap_pending: false,
            charsets: [Charset::Ascii; 4],
            origin: false,
        }
    }
}

/// What the terminal tells its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Bytes to write back to the program, such as answers to its questions.
    Reply(Vec<u8>),
    /// The title, or none to reset it.
    Title(Option<String>),
    /// The bell rang.
    Bell,
    /// The program put text on a clipboard (OSC 52): `c` the clipboard, `p` or `s` the selection.
    ClipboardStore(u8, String),
    /// The program asked to read a clipboard (OSC 52 with `?`); only when the owner allows it. The answer is
    /// `ESC ] 52 ; <clipboard> ; <base64> <terminator>`.
    ClipboardLoad(u8, &'static str),
    /// The program asked for the text area's size in pixels (CSI 14 t), which only the owner knows.
    TextAreaSizeRequest,
}

/// One terminal: parser, screens, cursor and modes.
#[derive(Debug)]
pub struct Terminal {
    parser: Parser,
    /// The screen in use.
    grid: Grid,
    /// The other screen: the alternate while the main one shows, and the other way round.
    inactive: Grid,
    cursor: Cursor,
    saved: Cursor,
    /// The other screen's cursor and saved cursor: each screen keeps its own.
    inactive_cursor: Cursor,
    inactive_saved: Cursor,
    modes: Modes,
    /// The scrolling region, top line inclusive to bottom line exclusive.
    region: std::ops::Range<usize>,
    tabs: Vec<bool>,
    active_charset: usize,
    styles: Styles,
    /// The last printed character, which REP repeats.
    preceding: Option<char>,
    title: Option<String>,
    events: Vec<Event>,
    size: Size,
    /// The kitty keyboard protocol's flags, as a stack the program pushes and pops.
    keyboard: Vec<u16>,
    /// What XTVERSION answers: the product's name and version.
    identity: String,
    /// The number of the cursor's style, while it is known; SGR and anything that swaps the cursor forget it.
    cursor_style_id: Option<StyleId>,
    /// How many rows have to be written anew before a full style table is compacted again.
    compact_after: u64,
    /// Shell integration marks, oldest first.
    marks: std::collections::VecDeque<Mark>,
    /// The number the next mark gets; it also changes when a bookmark goes.
    next_mark: u64,
    /// The working directory the shell reported last (OSC 7).
    directory: Option<Arc<crate::scan::WorkingDirectory>>,
    /// The colours the program changed.
    colors: Vec<Option<(u8, u8, u8)>>,
    /// The theme's colours, for answers.
    base_colors: Vec<(u8, u8, u8)>,
    /// The cursor's shape and blinking the program asked for (DECSCUSR); none for the user's choice.
    cursor_shape: Option<(CursorShape, bool)>,
    /// Programs may read the clipboard through OSC 52.
    clipboard_read: bool,
    /// Titles pushed with CSI 22 t, which CSI 23 t brings back.
    title_stack: Vec<Option<String>>,
}

/// The cursor's shape as a program asks for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// A filled cell.
    Block,
    /// A line under the cell.
    Underline,
    /// A bar before the cell.
    Bar,
}

/// Tab stops every this many columns, until a program sets its own.
const TAB_WIDTH: usize = 8;

impl Terminal {
    /// A terminal of `size` that keeps `history` lines of history.
    pub fn new(size: Size, history: usize) -> Terminal {
        let size = Size {
            columns: size.columns.max(2),
            lines: size.lines.max(1),
        };
        Terminal {
            parser: Parser::new(),
            grid: Grid::new(size.lines, size.columns, history),
            inactive: Grid::new(size.lines, size.columns, 0),
            cursor: Cursor::default(),
            saved: Cursor::default(),
            inactive_cursor: Cursor::default(),
            inactive_saved: Cursor::default(),
            modes: Modes::default(),
            region: 0..size.lines,
            tabs: tab_stops(size.columns),
            active_charset: 0,
            styles: Styles::default(),
            preceding: None,
            title: None,
            events: Vec::new(),
            size,
            keyboard: Vec::new(),
            identity: "term-vt".into(),
            cursor_style_id: Some(StyleId(0)),
            compact_after: 0,
            marks: std::collections::VecDeque::new(),
            next_mark: 0,
            directory: None,
            colors: vec![None; colors::COUNT],
            base_colors: vec![(0, 0, 0); colors::COUNT],
            cursor_shape: None,
            clipboard_read: false,
            title_stack: Vec::new(),
        }
    }

    /// Parses `bytes` of the program's output.
    pub fn advance(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        parser.advance(self, bytes);
        self.parser = parser;
    }

    /// Resizes to `size`, rewrapping the main screen's lines; the scrolling region resets to the whole screen.
    pub fn resize(&mut self, size: Size) {
        let size = Size {
            columns: size.columns.max(2),
            lines: size.lines.max(1),
        };
        if size == self.size {
            return;
        }
        let alt = self.modes.contains(Modes::ALT_SCREEN);
        let place = |cursor: &Cursor| crate::resize::Place {
            line: cursor.line,
            column: cursor.column,
            wrap_pending: cursor.wrap_pending,
        };
        let styles = self.styles.clone();
        let empty = |row: &Row, column: usize| is_empty_with(&styles, row, column);
        for (grid, cursor, saved, reflow) in [
            (&mut self.grid, &mut self.cursor, &mut self.saved, !alt),
            (
                &mut self.inactive,
                &mut self.inactive_cursor,
                &mut self.inactive_saved,
                alt,
            ),
        ] {
            let mut at = place(cursor);
            let mut saved_at = place(saved);
            grid.resize(reflow, size.lines, size.columns, &mut at, &mut saved_at, &empty);
            cursor.line = at.line;
            cursor.column = at.column;
            cursor.wrap_pending = at.wrap_pending;
            saved.line = saved_at.line;
            saved.column = saved_at.column;
        }
        let old = self.tabs.len();
        self.tabs.resize(size.columns, false);
        for (column, tab) in self.tabs.iter_mut().enumerate().skip(old) {
            *tab = column % TAB_WIDTH == 0;
        }
        self.size = size;
        self.region = 0..size.lines;
    }

    /// Has XTVERSION answer with `name` and `version` (E15).
    pub fn set_identity(&mut self, name: &str, version: &str) {
        self.identity = format!("{name} {version}");
    }

    /// Lets programs read the clipboard through OSC 52, which any program, also on a remote host, could misuse.
    pub fn allow_clipboard_read(&mut self, allowed: bool) {
        self.clipboard_read = allowed;
    }

    /// The cursor's shape and whether it blinks, as the program asked; none for the user's choice.
    pub fn cursor_shape(&self) -> Option<(CursorShape, bool)> {
        self.cursor_shape
    }

    /// The kitty keyboard protocol's flags in force.
    pub fn keyboard_protocol(&self) -> u16 {
        self.keyboard.last().copied().unwrap_or(0)
    }

    /// What happened since this was asked last.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// The screen in use and its history.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// The screen in use and its history, for scrolling the view.
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// What a terminal's Clear command does: the view back to the live screen, with `above_cursor` the lines above
    /// the cursor's scrolled away so that its line is at the top, and the history forgotten; the program is not told.
    pub fn clear_for_user(&mut self, above_cursor: bool) {
        self.grid.display_offset = 0;
        let line = self.cursor.line;
        if above_cursor && line > 0 {
            self.scroll_up(line);
            self.move_up(line);
        }
        self.grid.clear_history();
    }

    /// The cursor.
    pub fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    /// The modes the program set.
    pub fn modes(&self) -> Modes {
        self.modes
    }

    /// The title the program set last.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// The style numbered `id`.
    pub fn style(&self, id: StyleId) -> Style {
        self.styles.get(id)
    }

    /// The number of `style`, compacting the table when it is full; the default style while a table that is still
    /// full after compacting waits for a screen of rows to be written anew, as only that frees styles.
    fn style_id(&mut self, style: Style) -> StyleId {
        if let Some(id) = self.styles.intern(style) {
            return id;
        }
        let written = self.grid.rows_written() + self.inactive.rows_written();
        if written >= self.compact_after {
            self.compact_styles();
            self.compact_after = if self.styles.len() > STYLES_CROWDED {
                written + self.grid.lines() as u64
            } else {
                0
            };
        }
        self.styles.intern(style).unwrap_or_default()
    }

    /// Keeps only the styles a cell still uses, and renumbers the cells.
    fn compact_styles(&mut self) {
        self.cursor_style_id = None;
        let mut used = vec![false; self.styles.len()];
        let rows = self
            .grid
            .rows
            .iter()
            .chain(self.inactive.rows.iter())
            .map(Cow::Borrowed)
            .chain(self.grid.history.iter());
        for row in rows {
            for cell in &row.cells {
                used[usize::from(cell.style.0)] = true;
            }
        }
        let (styles, map) = self.styles.compact(&used);
        let renumber = |row: &mut Row| row.renumber_styles(&map);
        self.grid.rows.each_mut().for_each(renumber);
        self.grid.history.renumber_styles(&map);
        self.inactive.rows.each_mut().for_each(renumber);
        self.styles = styles;
    }

    /// Packs up to `most` of the history's older rows (F8), keeping the newest and those in view open; how many it
    /// packed, 0 once nothing is left to pack.
    pub fn pack_history(&mut self, most: usize) -> usize {
        let keep = (self.grid.display_offset + self.grid.lines()).max(OPEN_ROWS);
        self.grid.history.pack(keep, most)
    }

    /// The style id of a blank cell in the cursor's background, as erasing leaves it.
    fn blank(&mut self) -> StyleId {
        let bg = self.cursor.style.bg;
        if bg == Color::BACKGROUND {
            return StyleId(0);
        }
        self.style_id(Style::blank(bg))
    }

    /// Whether cell `column` of `row` holds nothing worth keeping when the screen is cleared.
    fn is_empty(&self, row: &Row, column: usize) -> bool {
        is_empty_with(&self.styles, row, column)
    }
}

/// Whether cell `column` of `row` holds nothing worth keeping, as Alacritty's `Cell::is_empty` decides.
fn is_empty_with(styles: &Styles, row: &Row, column: usize) -> bool {
    use crate::cell::CellFlags;
    use crate::style::Attrs;
    {
        let cell = &row.cells[column];
        let style = styles.get(cell.style);
        (cell.c == ' ' || cell.c == '\t')
            && style.bg == Color::BACKGROUND
            && style.fg == Color::FOREGROUND
            && !style.attrs.contains(Attrs::INVERSE)
            && style.attrs.0 & (Attrs::UNDERLINES.0 | Attrs::STRIKE.0) == 0
            && !cell.flags.intersects(CellFlags(
                CellFlags::WRAPPED.0 | CellFlags::WIDE_SPACER.0 | CellFlags::LEADING_WIDE_SPACER.0,
            ))
            && row.extra(column).is_none_or(|extra| extra.zero_width.is_empty())
    }
}

/// Tab stops every `TAB_WIDTH` columns.
fn tab_stops(columns: usize) -> Vec<bool> {
    (0..columns).map(|column| column % TAB_WIDTH == 0).collect()
}
