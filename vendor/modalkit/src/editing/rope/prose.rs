//! Dango: Vim's paragraph and sentence motions `{`, `}`, `(` and `)`, which `modalkit` left unimplemented. They walk
//! the text as Vim's `findpar()` and `findsent()` do, so that counts, empty lines and the end of the text land where
//! Vim lands.

use super::EditRope;
use crate::editing::cursor::Cursor;
use crate::prelude::{EditRange, MoveDir1D, TargetShape};

/// A line and a column in characters; the column after a line's last character stands for its end, Vim's NUL.
type Pos = (usize, usize);

/// What a step of Vim's `inc()` and `dec()` did: moved within the line, moved onto the end of the line, moved to
/// another line, or found no further place.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moved {
    Within,
    ToEnd,
    Line,
    Stuck,
}

/// Where a sentence ends, as Vim reads `.`, `!` and `?` followed by closing brackets and quotes.
const CLOSERS: &str = ")]\"'";
const ENDS: &str = ".!?";

impl EditRope {
    /// Where `count` paragraphs in `dir` from `cursor` lead, or `None` where the text has fewer; `motion` puts the
    /// cursor on the last character where the motion runs into the end of the text, an operator takes it along.
    pub(super) fn paragraph(&self, cursor: &Cursor, dir: MoveDir1D, count: usize, motion: bool) -> Option<Cursor> {
        let last = self.get_lines().checked_sub(1)?;
        let forward = dir == MoveDir1D::Next;
        let mut curr = cursor.y.min(last);
        for remaining in (0..count).rev() {
            let mut skipped = false;
            let mut first = true;
            loop {
                let empty = self.get_columns(curr) == 0;
                skipped |= !empty;
                if !first && skipped && empty {
                    break;
                }
                first = false;
                match (forward, curr) {
                    (true, line) if line < last => curr += 1,
                    (false, line) if line > 0 => curr -= 1,
                    _ if remaining > 0 => return None,
                    _ => break,
                }
            }
        }
        let columns = self.get_columns(curr);
        if forward && curr == last && columns > 0 {
            return Some(Cursor::new(curr, if motion { columns - 1 } else { columns }));
        }
        Some(Cursor::new(curr, 0))
    }

    /// Where `count` sentences in `dir` from `cursor` lead, or `None` where the text has fewer.
    pub(super) fn sentence(&self, cursor: &Cursor, dir: MoveDir1D, count: usize, motion: bool) -> Option<Cursor> {
        self.get_lines().checked_sub(1)?;
        let forward = dir == MoveDir1D::Next;
        let mut pos = (cursor.y, cursor.x.min(self.get_columns(cursor.y)));
        let mut count = count;
        while count > 0 {
            count -= 1;
            let prev = pos;
            let skip = self.sentence_step(&mut pos, forward, count)?;
            while skip && matches!(self.char_at(pos), Some(' ' | '\t')) {
                if self.incl(&mut pos) == Moved::Stuck {
                    break;
                }
            }
            if pos == prev {
                if self.walk(&mut pos, forward) == Moved::Stuck {
                    if count > 0 {
                        return None;
                    }
                    break;
                }
                count += 1;
            }
        }
        let columns = self.get_columns(pos.0);
        if motion && columns > 0 && pos.1 >= columns {
            pos.1 = columns - 1;
        }
        Some(Cursor::new(pos.0, pos.1))
    }

    /// One sentence of Vim's `findsent()` from `pos`, up to its label `found`; whether white space after it is
    /// skipped, or `None` where the text ends with `remaining` sentences still to go.
    fn sentence_step(&self, pos: &mut Pos, forward: bool, remaining: usize) -> Option<bool> {
        if self.char_at(*pos).is_none() {
            loop {
                if self.walk(pos, forward) == Moved::Stuck || self.char_at(*pos).is_some() {
                    break;
                }
            }
            if forward {
                return Some(true);
            }
        } else if !forward {
            self.decl(pos);
        }
        self.back_to_text(pos, forward);
        let start_line = pos.0;
        loop {
            let Some(c) = self.char_at(*pos) else {
                if !forward && pos.0 != start_line {
                    *pos = (pos.0 + 1, 0);
                }
                return Some(true);
            };
            if ENDS.contains(c) {
                let mut after = *pos;
                let mut moved;
                loop {
                    moved = self.inc(&mut after);
                    if moved == Moved::Stuck || !self.char_at(after).is_some_and(|c| CLOSERS.contains(c)) {
                        break;
                    }
                }
                if moved == Moved::Stuck || matches!(self.char_at(after), None | Some(' ' | '\t')) {
                    *pos = after;
                    if self.char_at(*pos).is_none() {
                        self.inc(pos);
                    }
                    return Some(true);
                }
            }
            if self.walk(pos, forward) == Moved::Stuck {
                if remaining > 0 {
                    return None;
                }
                return Some(false);
            }
        }
    }

    /// Goes back over white space and the marks that end a sentence to the text before them, as `findsent()` does
    /// before it looks for the end.
    fn back_to_text(&self, pos: &mut Pos, forward: bool) {
        let mut dot = false;
        while let Some(c) = self
            .char_at(*pos)
            .filter(|c| matches!(c, ' ' | '\t') || ENDS.contains(*c) || CLOSERS.contains(*c))
        {
            let mut before = *pos;
            if self.decl(&mut before) == Moved::Stuck || (self.get_columns(before.0) == 0 && forward) {
                break;
            }
            if dot {
                break;
            }
            dot |= ENDS.contains(c);
            if CLOSERS.contains(c)
                && !self
                    .char_at(before)
                    .is_some_and(|b| ENDS.contains(b) || CLOSERS.contains(b))
            {
                break;
            }
            self.decl(pos);
        }
    }

    /// The character at `pos`, or `None` at the end of its line.
    fn char_at(&self, (line, column): Pos) -> Option<char> {
        (column < self.get_columns(line)).then(|| self.rope.char(self.rope.line_to_char(line) + column))
    }

    fn walk(&self, pos: &mut Pos, forward: bool) -> Moved {
        if forward { self.incl(pos) } else { self.decl(pos) }
    }

    /// Vim's `inc()`: the next character, the end of the line, or the start of the next line.
    fn inc(&self, pos: &mut Pos) -> Moved {
        let columns = self.get_columns(pos.0);
        if pos.1 < columns {
            pos.1 += 1;
            return if pos.1 < columns { Moved::Within } else { Moved::ToEnd };
        }
        if pos.0 + 1 < self.get_lines() {
            *pos = (pos.0 + 1, 0);
            return Moved::Line;
        }
        Moved::Stuck
    }

    /// Vim's `incl()`: as `inc()`, but over the end of a line that is not empty.
    fn incl(&self, pos: &mut Pos) -> Moved {
        let moved = self.inc(pos);
        if matches!(moved, Moved::ToEnd | Moved::Line) && pos.1 > 0 {
            self.inc(pos)
        } else {
            moved
        }
    }

    /// Vim's `dec()`: the character before, or the end of the line before.
    fn dec(&self, pos: &mut Pos) -> Moved {
        if pos.1 > 0 {
            pos.1 -= 1;
            return Moved::Within;
        }
        if pos.0 > 0 {
            *pos = (pos.0 - 1, self.get_columns(pos.0 - 1));
            return Moved::Line;
        }
        Moved::Stuck
    }

    /// Vim's `decl()`: as `dec()`, but over the end of a line that is not empty.
    fn decl(&self, pos: &mut Pos) -> Moved {
        let moved = self.dec(pos);
        if moved == Moved::Line && pos.1 > 0 {
            self.dec(pos)
        } else {
            moved
        }
    }

    /// `range`, of an operator with an exclusive motion, as Vim's `:help exclusive` corrects it: one that ends at the
    /// start of a later line ends at the end of the line before instead, and takes whole lines when it starts at or
    /// before the first non-blank character of its own.
    pub(super) fn exclusive_rules(&self, mut range: EditRange<Cursor>) -> EditRange<Cursor> {
        if range.inclusive || range.end.x != 0 || range.end.y <= range.start.y {
            return range;
        }
        let line = range.end.y - 1;
        let first = (0..self.get_columns(range.start.y))
            .find(|column| !matches!(self.char_at((range.start.y, *column)), Some(' ' | '\t')))
            .unwrap_or(0);
        if range.start.x <= first {
            return EditRange::inclusive(range.start, Cursor::new(line, 0), TargetShape::LineWise);
        }
        range.end = Cursor::new(line, self.get_columns(line));
        range
    }
}
