//! Copy mode without AppKit (F111): which keys move, select, search, copy or leave, carried out by `term-vim` on a
//! copy of the session's rows, and its cursor and selection turned back into rows and cells.

use std::collections::HashMap;

use term_session::SelectionKind;
use term_session::session::RowsText;
use term_vim::{Effect, Mode, Note, Vim};

/// A key as copy mode reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyKey {
    Char(char),
    /// A letter with ⌃.
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// A row's number and a cell in it.
pub(crate) type Place = (u64, usize);

/// What a key asks of the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The cursor, the selection or the prompt may have changed.
    Moved,
    /// Copy the selection, or the cursor's row without one, and leave.
    Copy,
    /// Leave without copying.
    Leave,
    /// The key does nothing here.
    Ignored,
}

/// A key that `term-vim` waits for after the last one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Awaiting {
    #[default]
    Nothing,
    /// Any character, as after `f` or after `i` in Visual mode.
    AnyChar,
    /// What follows `g`.
    AfterG,
}

/// Keys of Normal and Visual mode that only move; digits are counts.
const MOVES: &str = "hjklwbeWBE0^$G;,nN*#{}%()|_+- 123456789";
/// Keys that take the next character as their argument.
const TAKES_CHAR: &str = "fFtT";
/// What may follow `g` and only moves.
const AFTER_G: &str = "geE_jk0^$m";

/// Copy mode's state over a copy of the rows.
pub(crate) struct CopyMode {
    vim: Vim,
    cells: HashMap<usize, Vec<usize>>,
    first: u64,
    rows: usize,
    /// The rows' text as copy mode took it last, so that output that changed none of it leaves copy mode alone.
    text: Vec<String>,
    awaiting: Awaiting,
    note: Option<Note>,
}

/// What the status shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) mode: Mode,
    /// The search being typed after `/` or `?`.
    pub(crate) prompt: Option<String>,
    /// Why the last key could not do what it asked.
    pub(crate) note: Option<Note>,
}

impl CopyMode {
    /// Copy mode over `text`, its cursor on row `row` at cell `cell`, an index into the rows.
    pub(crate) fn new(text: RowsText, (row, cell): (usize, usize)) -> CopyMode {
        let rows = text.rows.len().max(1);
        let row = row.min(rows - 1);
        let length = text.rows.get(row).map_or(0, |r| r.chars().count());
        let mut vim = Vim::new(&text.rows.join("\n"));
        let column = char_of(&text.cells, row, cell).min(length.saturating_sub(1));
        vim.set_cursor(row, column);
        CopyMode {
            vim,
            cells: text.cells,
            first: text.first,
            rows,
            text: text.rows,
            awaiting: Awaiting::Nothing,
            note: None,
        }
    }

    /// Takes the session's rows anew after output, the cursor and the selection staying on the rows they were on.
    pub(crate) fn refresh(&mut self, text: RowsText) {
        if text.first == self.first && text.rows == self.text {
            return;
        }
        let gone = usize::try_from(text.first.saturating_sub(self.first)).unwrap_or(usize::MAX);
        self.vim.replace_lines(&text.rows.join("\n"), gone);
        self.rows = text.rows.len().max(1);
        self.cells = text.cells;
        self.first = text.first;
        self.text = text.rows;
    }

    /// Carries out `key`; `page` is how many rows the view shows.
    pub(crate) fn key(&mut self, key: CopyKey, page: usize) -> Step {
        self.note = None;
        if self.vim.mode() == Mode::Prompt {
            return self.prompt_key(key);
        }
        match (std::mem::take(&mut self.awaiting), key) {
            (Awaiting::AnyChar, CopyKey::Char(c)) => return self.feed_char(c),
            (Awaiting::AnyChar, _) => return self.feed("<Esc>"),
            (Awaiting::AfterG, CopyKey::Char(c)) if AFTER_G.contains(c) => return self.feed_char(c),
            _ => {}
        }
        let visual = matches!(self.vim.mode(), Mode::Visual | Mode::VisualLine | Mode::VisualBlock);
        let half = (page / 2).max(1);
        match key {
            CopyKey::Char('q') => Step::Leave,
            CopyKey::Escape if visual => self.feed("<Esc>"),
            CopyKey::Escape => Step::Leave,
            CopyKey::Char('y') | CopyKey::Enter => Step::Copy,
            CopyKey::Ctrl('u') => self.jump(-signed(half)),
            CopyKey::Ctrl('d') => self.jump(signed(half)),
            CopyKey::Ctrl('b') | CopyKey::PageUp => self.jump(-signed(page.max(1))),
            CopyKey::Ctrl('f') | CopyKey::PageDown => self.jump(signed(page.max(1))),
            CopyKey::Ctrl('v') => self.feed("<C-V>"),
            CopyKey::Ctrl(_) => Step::Ignored,
            CopyKey::Up => self.feed_char('k'),
            CopyKey::Down => self.feed_char('j'),
            CopyKey::Left | CopyKey::Backspace => self.feed_char('h'),
            CopyKey::Right => self.feed_char('l'),
            CopyKey::Home => self.feed_char('0'),
            CopyKey::End => self.feed_char('$'),
            CopyKey::Char(c @ ('v' | 'V' | '/' | '?')) => self.feed_char(c),
            CopyKey::Char(c @ ('o' | 'O')) if visual => self.feed_char(c),
            CopyKey::Char(c @ ('i' | 'a')) if visual => self.awaiting(Awaiting::AnyChar, c),
            CopyKey::Char(c) if TAKES_CHAR.contains(c) => self.awaiting(Awaiting::AnyChar, c),
            CopyKey::Char('g') => self.awaiting(Awaiting::AfterG, 'g'),
            CopyKey::Char(c) if MOVES.contains(c) => self.feed_char(c),
            CopyKey::Char(_) => Step::Ignored,
        }
    }

    fn prompt_key(&mut self, key: CopyKey) -> Step {
        let name = match key {
            CopyKey::Char(c) => return self.feed_char(c),
            CopyKey::Escape => "<Esc>",
            CopyKey::Enter => "<Enter>",
            CopyKey::Backspace => "<BS>",
            CopyKey::Left => "<Left>",
            CopyKey::Right => "<Right>",
            CopyKey::Up => "<Up>",
            CopyKey::Down => "<Down>",
            CopyKey::Ctrl('u') => "<C-U>",
            CopyKey::Ctrl('w') => "<C-W>",
            _ => return Step::Ignored,
        };
        self.feed(name)
    }

    fn awaiting(&mut self, next: Awaiting, c: char) -> Step {
        self.awaiting = next;
        self.feed_char(c)
    }

    fn feed_char(&mut self, c: char) -> Step {
        let outcome = self.vim.char(c);
        self.take(outcome.effects)
    }

    fn feed(&mut self, keys: &str) -> Step {
        let outcome = self.vim.keys(keys);
        self.take(outcome.effects)
    }

    fn take(&mut self, effects: Vec<Effect>) -> Step {
        for effect in effects {
            if let Effect::Note(note) = effect {
                self.note = Some(note);
            }
        }
        Step::Moved
    }

    /// Moves the cursor `rows` rows down, or up when negative, keeping its column as far as the row reaches.
    fn jump(&mut self, rows: i64) -> Step {
        let (row, column) = self.vim.cursor();
        let last = signed(self.rows - 1);
        let target = usize::try_from((signed(row) + rows).clamp(0, last)).unwrap_or(0);
        self.vim.set_cursor(target, column);
        Step::Moved
    }

    /// The cursor as a row's number and a cell.
    pub(crate) fn cursor(&mut self) -> Place {
        let (row, column) = self.vim.cursor();
        (self.number(row), cell_of(&self.cells, row, column))
    }

    /// How many cells the character under the cursor takes.
    pub(crate) fn cursor_width(&mut self) -> usize {
        let (row, column) = self.vim.cursor();
        self.cells
            .get(&row)
            .and_then(|cells| Some(cells.get(column + 1)? - cells.get(column)?))
            .map_or(1, |width| width.clamp(1, 2))
    }

    /// The selection as its kind and its first and last cell, each a row's number and a cell; none outside Visual
    /// mode.
    pub(crate) fn selection(&mut self) -> Option<(SelectionKind, Place, Place)> {
        let ((top, from), (bottom, to)) = self.vim.selection()?;
        let (top_row, bottom_row) = (self.number(top), self.number(bottom));
        let (from, to) = (cell_of(&self.cells, top, from), cell_of(&self.cells, bottom, to));
        Some(match self.vim.mode() {
            Mode::VisualLine => (SelectionKind::Line, (top_row, 0), (bottom_row, 0)),
            Mode::VisualBlock => (
                SelectionKind::Block,
                (top_row, from.min(to)),
                (bottom_row, from.max(to)),
            ),
            _ => (SelectionKind::Simple, (top_row, from), (bottom_row, to)),
        })
    }

    /// What the status shows now.
    pub(crate) fn status(&self) -> Status {
        Status {
            mode: self.vim.mode(),
            prompt: self.vim.prompt(),
            note: self.note.clone(),
        }
    }

    fn number(&self, row: usize) -> u64 {
        self.first + row as u64
    }
}

fn signed(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// How many of the oldest rows of `old`, whose first `history` are the history's, `new` no longer has: the least
/// that lets the history go on where `new` begins.
/// The cell of character `column` of row `row`.
fn cell_of(cells: &HashMap<usize, Vec<usize>>, row: usize, column: usize) -> usize {
    match cells.get(&row) {
        Some(places) => match places.get(column) {
            Some(cell) => *cell,
            None => places.last().map_or(column, |last| last + 1 + column - places.len()),
        },
        None => column,
    }
}

/// The character of row `row` at cell `cell`, or the one whose cell it is the second half of.
fn char_of(cells: &HashMap<usize, Vec<usize>>, row: usize, cell: usize) -> usize {
    match cells.get(&row) {
        Some(places) => {
            let base = places.iter().copied().filter(|place| *place <= cell).max();
            base.and_then(|base| places.iter().position(|place| *place == base))
                .unwrap_or(0)
        }
        None => cell,
    }
}

/// The keys of `keys` written as Vim writes them: characters, and names such as `<Esc>`, `<Enter>` and `<C-v>`.
#[cfg(any(test, feature = "selftest"))]
pub(crate) fn parse(keys: &str) -> Vec<CopyKey> {
    let mut out = Vec::new();
    let mut rest = keys;
    while let Some(first) = rest.chars().next() {
        let named = (first == '<')
            .then(|| rest.find('>'))
            .flatten()
            .and_then(|end| Some((named_key(&rest[1..end])?, end + 1)));
        if let Some((key, length)) = named {
            out.push(key);
            rest = &rest[length..];
        } else {
            out.push(CopyKey::Char(first));
            rest = &rest[first.len_utf8()..];
        }
    }
    out
}

#[cfg(any(test, feature = "selftest"))]
fn named_key(name: &str) -> Option<CopyKey> {
    if let Some(letter) = name.strip_prefix("C-") {
        let mut chars = letter.chars();
        return match (chars.next(), chars.next()) {
            (Some(c), None) => Some(CopyKey::Ctrl(c.to_ascii_lowercase())),
            _ => None,
        };
    }
    Some(match name {
        "Esc" => CopyKey::Escape,
        "Enter" | "CR" => CopyKey::Enter,
        "BS" => CopyKey::Backspace,
        "Up" => CopyKey::Up,
        "Down" => CopyKey::Down,
        "Left" => CopyKey::Left,
        "Right" => CopyKey::Right,
        "Home" => CopyKey::Home,
        "End" => CopyKey::End,
        "PageUp" => CopyKey::PageUp,
        "PageDown" => CopyKey::PageDown,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
