//! The session's screen on the terminal's own core (E4): what it shows as a `Snapshot`, the selection, scrolling and
//! input modes, taken from `term_vt`.

use std::sync::Arc;

use term_vt::Terminal;
use term_vt::cell::CellFlags as VtFlags;
use term_vt::color::Color;
use term_vt::selection::{Anchor, Kind, Selection, Side};
use term_vt::style::Attrs;
use term_vt::term::{CursorShape as VtShape, Modes};

use crate::input::{InputModes, KittyFlags, MouseEncoding, MouseTracking};
use crate::session::{CellSide, Scroll, SelectionKind};
use crate::snapshot::{Cell, CellFlags, Cursor, CursorShape, Damage, Highlight, Palette, Point, Rgb, Snapshot};

mod blocks;
mod fold;
mod prompt;
mod styled;

/// The own core with what the session adds around it.
#[derive(Debug)]
pub(crate) struct Own {
    pub(crate) terminal: Terminal,
    pub(crate) selection: Option<Selection>,
    pub(crate) focused: bool,
    /// When the program began a synchronized update (mode 2026) that has not ended.
    pub(crate) sync_since: Option<std::time::Instant>,
    /// The search the user runs, with its matches.
    pub(crate) search: Option<Search>,
    /// The command blocks and which of them are collapsed.
    pub(crate) blocks: blocks::Blocks,
    /// The input field stands in for the live prompt, which the view then leaves out (E7).
    pub(crate) hide_prompt: bool,
}

/// A search with its matches and the one shown.
#[derive(Debug)]
pub(crate) struct Search {
    pattern: String,
    options: term_vt::search::Options,
    matches: Vec<term_vt::search::Match>,
    current: Option<usize>,
}

/// The most matches a search counts.
const MOST_MATCHES: usize = 10_000;

impl Own {
    pub(crate) fn new(terminal: Terminal) -> Own {
        Own {
            terminal,
            selection: None,
            focused: true,
            sync_since: None,
            search: None,
            blocks: blocks::Blocks::default(),
            hide_prompt: false,
        }
    }

    fn colour(&self, palette: &Palette, index: usize) -> Rgb {
        if let Some(Some((r, g, b))) = self.terminal.color_changes().get(index) {
            return Rgb::new(*r, *g, *b);
        }
        match index {
            0..=255 => palette.indexed(u8::try_from(index).unwrap_or(0)),
            term_vt::term::colors::FOREGROUND => palette.foreground,
            term_vt::term::colors::BACKGROUND => palette.background,
            _ => palette.cursor.unwrap_or(palette.foreground),
        }
    }

    fn foreground(&self, palette: &Palette, color: Color, attrs: Attrs) -> Rgb {
        let bold = attrs.contains(Attrs::BOLD) && palette.bold_is_bright;
        let dim = attrs.contains(Attrs::DIM);
        let index = |i: usize| self.colour(palette, i);
        match color {
            Color::Rgb(r, g, b) => {
                let rgb = Rgb::new(r, g, b);
                if dim { rgb.dim() } else { rgb }
            }
            Color::Named(256) | Color::Named(258..) => {
                let rgb = index(term_vt::term::colors::FOREGROUND);
                if dim { rgb.dim() } else { rgb }
            }
            Color::Named(257) => index(term_vt::term::colors::BACKGROUND),
            Color::Named(n @ 0..=15) => {
                let n = usize::from(n);
                match n {
                    0..=7 if dim => index(n).dim(),
                    8..=15 if dim => index(n - 8),
                    0..=7 if bold => index(n + 8),
                    _ => index(n),
                }
            }
            Color::Indexed(n @ 0..=15) => {
                let n = usize::from(n);
                match n {
                    0..=7 if dim => index(n).dim(),
                    8..=15 if dim => index(n - 8),
                    0..=7 if bold => index(n + 8),
                    _ => index(n),
                }
            }
            Color::Indexed(i) => index(usize::from(i)),
            Color::Named(n) => index(usize::from(n).min(255)),
        }
    }

    fn background(&self, palette: &Palette, color: Color) -> Rgb {
        match color {
            Color::Rgb(r, g, b) => Rgb::new(r, g, b),
            Color::Named(257) => self.colour(palette, term_vt::term::colors::BACKGROUND),
            Color::Named(256) => self.colour(palette, term_vt::term::colors::FOREGROUND),
            Color::Named(n) => self.colour(palette, usize::from(n).min(255)),
            Color::Indexed(i) => self.colour(palette, usize::from(i)),
        }
    }

    /// The visible screen at this moment.
    pub(crate) fn snapshot(&self, palette: &Palette) -> Snapshot {
        let terminal = &self.terminal;
        let grid = terminal.grid();
        let size = terminal.size();
        let mut rows = Vec::with_capacity(size.lines);
        let shown = self.shown_lines();
        for line in 0..size.lines {
            let row = match &shown {
                Some(shown) => shown.get(line).and_then(|&global| grid.row_global(global)),
                None => grid.view_row(line),
            };
            let Some(row) = row else {
                let blank = Cell {
                    ch: ' ',
                    zero_width: Vec::new(),
                    fg: palette.foreground,
                    bg: palette.background,
                    underline_color: None,
                    flags: CellFlags::default(),
                    hyperlink: None,
                };
                rows.push(vec![blank; size.columns]);
                continue;
            };
            let mut cells = Vec::with_capacity(size.columns);
            for (column, cell) in row.cells.iter().enumerate() {
                let style = terminal.style(cell.style);
                let mut fg = self.foreground(palette, style.fg, style.attrs);
                let mut bg = self.background(palette, style.bg);
                if style.attrs.contains(Attrs::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let extra = cell.flags.contains(VtFlags::EXTRA).then(|| row.extra(column)).flatten();
                cells.push(Cell {
                    ch: cell.c,
                    zero_width: extra.map(|e| e.zero_width.chars().collect()).unwrap_or_default(),
                    fg,
                    bg,
                    underline_color: style.underline.map(|c| self.background(palette, c)),
                    flags: flags(cell.flags, style.attrs),
                    hyperlink: extra
                        .and_then(|e| e.link.as_ref())
                        .map(|link| Arc::from(link.uri.as_str())),
                });
            }
            rows.push(cells);
        }
        let cursor = terminal.cursor();
        let offset = grid.display_offset;
        let line = self.cursor_view_line(shown.as_deref(), cursor.line + offset);
        let (shape, blinking) = match terminal.cursor_shape() {
            Some((VtShape::Block, blink)) => (CursorShape::Block, blink),
            Some((VtShape::Underline, blink)) => (CursorShape::Underline, blink),
            Some((VtShape::Bar, blink)) => (CursorShape::Beam, blink),
            None => (CursorShape::Block, false),
        };
        let wide = grid
            .rows
            .get(cursor.line)
            .and_then(|row| row.cells.get(cursor.column))
            .is_some_and(|cell| cell.flags.contains(VtFlags::WIDE));
        Snapshot {
            columns: size.columns,
            lines: size.lines,
            rows,
            cursor: Cursor {
                point: Point {
                    line: line.min(size.lines - 1),
                    column: cursor.column,
                },
                shape,
                visible: terminal.modes().contains(Modes::SHOW_CURSOR) && line < size.lines,
                blinking,
                wide,
            },
            selection: self.highlight(),
            display_offset: offset,
            history: grid.history.len(),
            damage: Damage::Full,
            background: self.colour(palette, term_vt::term::colors::BACKGROUND),
            modes: self.input_modes(),
        }
    }

    fn highlight(&self) -> Option<Highlight> {
        if let Some(shown) = self.shown_lines() {
            return self.folded_highlight(&shown);
        }
        let selection = self.selection.as_ref()?;
        let grid = self.terminal.grid();
        let (first, last) = selection.range(grid)?;
        let top = grid.global_of_view(0);
        let bottom = top + grid.lines() as u64 - 1;
        if last.line < top || first.line > bottom {
            return None;
        }
        let point = |anchor: Anchor, clamp_start: bool| {
            if anchor.line < top {
                return Point { line: 0, column: 0 };
            }
            if anchor.line > bottom {
                return Point {
                    line: grid.lines() - 1,
                    column: grid.columns() - 1,
                };
            }
            let _ = clamp_start;
            Point {
                line: usize::try_from(anchor.line - top).unwrap_or(0),
                column: anchor.column,
            }
        };
        Some(Highlight {
            start: point(first, true),
            end: point(last, false),
            block: selection.kind == Kind::Block,
        })
    }

    pub(crate) fn input_modes(&self) -> InputModes {
        let modes = self.terminal.modes();
        let mouse = if modes.contains(Modes::MOUSE_MOTION) {
            MouseTracking::AnyMotion
        } else if modes.contains(Modes::MOUSE_DRAG) {
            MouseTracking::ButtonMotion
        } else if modes.contains(Modes::MOUSE_CLICK) {
            MouseTracking::Clicks
        } else {
            MouseTracking::Off
        };
        let mouse_encoding = if modes.contains(Modes::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if modes.contains(Modes::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Default
        };
        InputModes {
            app_cursor: modes.contains(Modes::APP_CURSOR),
            app_keypad: modes.contains(Modes::APP_KEYPAD),
            newline: modes.contains(Modes::LINE_FEED_NEW_LINE),
            bracketed_paste: modes.contains(Modes::BRACKETED_PASTE),
            focus_reporting: modes.contains(Modes::FOCUS),
            mouse,
            mouse_encoding,
            alternate_screen: modes.contains(Modes::ALT_SCREEN),
            alternate_scroll: modes.contains(Modes::ALTERNATE_SCROLL),
            kitty: KittyFlags(u8::try_from(self.terminal.keyboard_protocol() & 0x1f).unwrap_or(0)),
        }
    }

    pub(crate) fn visible_text(&self) -> Vec<String> {
        let grid = self.terminal.grid();
        let shown = self.shown_lines();
        (0..grid.lines())
            .map(|line| {
                let row = match &shown {
                    Some(_) => grid.row_global(self.global_of_view(shown.as_deref(), line)),
                    None => grid.view_row(line),
                };
                let mut text = row.map(|row| row.text()).unwrap_or_default();
                text.truncate(text.trim_end().len());
                text
            })
            .collect()
    }

    pub(crate) fn scroll(&mut self, scroll: Scroll) {
        if self.scroll_folded(scroll) {
            return;
        }
        let lines = isize::try_from(self.terminal.size().lines).unwrap_or(1);
        let grid = self.terminal.grid_mut();
        match scroll {
            Scroll::Lines(delta) => grid.scroll_view(isize::try_from(delta).unwrap_or(0)),
            Scroll::PageUp => grid.scroll_view(lines),
            Scroll::PageDown => grid.scroll_view(-lines),
            Scroll::Top => grid.scroll_view(isize::MAX / 2),
            Scroll::Bottom => grid.display_offset = 0,
        }
    }

    /// The anchor of view point `point`.
    fn anchor(&self, point: Point) -> Anchor {
        let grid = self.terminal.grid();
        Anchor {
            line: self.global_of_view(self.shown_lines().as_deref(), point.line),
            column: point.column.min(grid.columns() - 1),
        }
    }

    pub(crate) fn select(&mut self, kind: SelectionKind, point: Point, side: CellSide) {
        let kind = match kind {
            SelectionKind::Simple => Kind::Simple,
            SelectionKind::Block => Kind::Block,
            SelectionKind::Word => Kind::Word,
            SelectionKind::Line => Kind::Line,
        };
        let at = (self.anchor(point), side_of(side));
        self.selection = Some(Selection {
            kind,
            start: at,
            end: at,
        });
    }

    pub(crate) fn extend_selection(&mut self, point: Point, side: CellSide) {
        let at = (self.anchor(point), side_of(side));
        if let Some(selection) = self.selection.as_mut() {
            selection.end = at;
        }
    }

    /// Searches for `pattern` and shows the next match after the one shown, or the previous one: selected, and
    /// scrolled into view. How many matches there are and which one shows.
    pub(crate) fn find(
        &mut self,
        pattern: &str,
        options: term_vt::search::Options,
        forward: bool,
    ) -> Result<crate::session::Found, String> {
        let fresh = self
            .search
            .as_ref()
            .is_none_or(|search| search.pattern != pattern || search.options != options);
        if fresh {
            let expression = term_vt::search::compile(pattern, options).map_err(|error| error.to_string())?;
            let matches = term_vt::search::find_all(self.terminal.grid(), &expression, MOST_MATCHES);
            self.search = Some(Search {
                pattern: pattern.to_string(),
                options,
                matches,
                current: None,
            });
        }
        let grid = self.terminal.grid();
        let top = grid.global_of_view(0);
        let Some(search) = self.search.as_mut() else {
            return Ok(crate::session::Found::default());
        };
        let count = search.matches.len();
        if count == 0 || pattern.is_empty() {
            search.current = None;
            self.selection = None;
            return Ok(crate::session::Found { count, current: None });
        }
        let next = match (search.current, forward) {
            (Some(current), true) => (current + 1) % count,
            (Some(current), false) => (current + count - 1) % count,
            // A new search starts at the newest match at or above the view's bottom, looking back from what shows.
            (None, _) => {
                let bottom = top + grid.lines() as u64;
                search
                    .matches
                    .iter()
                    .rposition(|m| m.start.line < bottom)
                    .unwrap_or(count - 1)
            }
        };
        search.current = Some(next);
        let hit = search.matches[next];
        self.selection = Some(term_vt::selection::Selection {
            kind: term_vt::selection::Kind::Simple,
            start: (hit.start, term_vt::selection::Side::Left),
            end: (hit.end, term_vt::selection::Side::Right),
        });
        self.unfold_line(hit.start.line);
        let grid = self.terminal.grid_mut();
        if grid.view_of_global(hit.start.line).is_none() {
            let index = usize::try_from(hit.start.line.saturating_sub(grid.dropped)).unwrap_or(0);
            let middle = grid.lines() / 2;
            grid.display_offset = grid
                .history
                .len()
                .saturating_sub(index.saturating_sub(middle))
                .min(grid.history.len());
        }
        Ok(crate::session::Found {
            count,
            current: Some(next),
        })
    }

    /// The rows the search's matches start on.
    pub(crate) fn search_lines(&self) -> Vec<u64> {
        self.search
            .as_ref()
            .map(|search| search.matches.iter().map(|m| m.start.line).collect())
            .unwrap_or_default()
    }

    /// Ends the search and its highlight.
    pub(crate) fn end_find(&mut self) {
        if self.search.take().is_some() {
            self.selection = None;
        }
    }

    pub(crate) fn selected_text(&self) -> Option<String> {
        self.selection.as_ref()?.text(self.terminal.grid())
    }
}

fn side_of(side: CellSide) -> Side {
    match side {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

fn flags(cell: VtFlags, attrs: Attrs) -> CellFlags {
    let mut bits = CellFlags::default();
    let pairs = [
        (attrs.contains(Attrs::BOLD), CellFlags::BOLD),
        (attrs.contains(Attrs::ITALIC), CellFlags::ITALIC),
        (attrs.contains(Attrs::DIM), CellFlags::DIM),
        (attrs.contains(Attrs::UNDERLINE), CellFlags::UNDERLINE),
        (attrs.contains(Attrs::DOUBLE_UNDERLINE), CellFlags::DOUBLE_UNDERLINE),
        (attrs.contains(Attrs::UNDERCURL), CellFlags::UNDERCURL),
        (attrs.contains(Attrs::DOTTED_UNDERLINE), CellFlags::DOTTED_UNDERLINE),
        (attrs.contains(Attrs::DASHED_UNDERLINE), CellFlags::DASHED_UNDERLINE),
        (attrs.contains(Attrs::STRIKE), CellFlags::STRIKETHROUGH),
        (attrs.contains(Attrs::INVERSE), CellFlags::INVERSE),
        (attrs.contains(Attrs::HIDDEN), CellFlags::HIDDEN),
        (cell.contains(VtFlags::WIDE), CellFlags::WIDE),
        (
            cell.intersects(VtFlags(VtFlags::WIDE_SPACER.0 | VtFlags::LEADING_WIDE_SPACER.0)),
            CellFlags::WIDE_SPACER,
        ),
        (cell.contains(VtFlags::WRAPPED), CellFlags::WRAPPED),
    ];
    for (on, flag) in pairs {
        if on {
            bits.insert(flag);
        }
    }
    bits
}
