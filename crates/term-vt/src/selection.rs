//! A selection, anchored in global line numbers that stay with their rows as the history grows and drops its oldest
//! (E4): character by character, as a block, by words or by whole lines.

use crate::cell::CellFlags;
use crate::grid::Grid;

/// How a selection grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Cell by cell.
    Simple,
    /// A rectangle.
    Block,
    /// Whole words.
    Word,
    /// Whole lines.
    Line,
}

/// Which half of a cell a point is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The left half.
    Left,
    /// The right half.
    Right,
}

/// A place in a grid: a global line number and a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Anchor {
    /// The line, counted from the first row the grid ever had.
    pub line: u64,
    /// The column.
    pub column: usize,
}

/// A selection between two anchors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// How it grows.
    pub kind: Kind,
    /// Where it started.
    pub start: (Anchor, Side),
    /// Where it ends now.
    pub end: (Anchor, Side),
}

/// Characters that end a word, as Alacritty's default has them.
const WORD_ENDS: &str = ",│`|:\"' ()[]{}<>\t";

impl Selection {
    /// The selected cells, first to last inclusive, or none when the selection is empty; for a block, the corners.
    pub fn range(&self, grid: &Grid) -> Option<(Anchor, Anchor)> {
        let (mut a, mut b) = (self.start, self.end);
        if (b.0.line, b.0.column) < (a.0.line, a.0.column) {
            std::mem::swap(&mut a, &mut b);
        }
        let last_column = grid.columns().saturating_sub(1);
        match self.kind {
            Kind::Simple if a.0 == b.0 => (a.1 != b.1).then_some((a.0, a.0)),
            Kind::Simple => {
                let mut first = a.0;
                let mut last = b.0;
                if a.1 == Side::Right {
                    if first.column >= last_column {
                        first = Anchor {
                            line: first.line + 1,
                            column: 0,
                        };
                    } else {
                        first.column += 1;
                    }
                }
                if b.1 == Side::Left {
                    if last.column == 0 {
                        last = Anchor {
                            line: last.line.checked_sub(1)?,
                            column: last_column,
                        };
                    } else {
                        last.column -= 1;
                    }
                }
                (first <= last).then_some((first, last))
            }
            Kind::Block => block(a, b),
            Kind::Line => {
                let mut first = a.0.line;
                while first > 0 && grid.row_global(first - 1).is_some_and(|row| row.wrapped()) {
                    first -= 1;
                }
                let mut last = b.0.line;
                while grid.row_global(last).is_some_and(|row| row.wrapped()) && grid.row_global(last + 1).is_some() {
                    last += 1;
                }
                Some((
                    Anchor { line: first, column: 0 },
                    Anchor {
                        line: last,
                        column: last_column,
                    },
                ))
            }
            Kind::Word => Some((word_start(grid, a.0), word_end(grid, b.0))),
        }
    }

    /// The selected text: lines that wrapped join, others end in a newline; trailing blanks of a line are dropped.
    pub fn text(&self, grid: &Grid) -> Option<String> {
        let mut out = String::new();
        for piece in self.pieces(grid)? {
            let Some(row) = grid.row_global(piece.line) else {
                continue;
            };
            let mut text = String::new();
            for &column in &piece.columns {
                let c = row.cells[column].c;
                text.push(if c == '\t' { ' ' } else { c });
                if let Some(extra) = row.extra(column) {
                    text.push_str(&extra.zero_width);
                }
            }
            if piece.joined {
                out.push_str(&text);
            } else {
                out.push_str(text.trim_end());
                if piece.newline {
                    out.push('\n');
                }
            }
        }
        Some(out)
    }

    /// The selected cells row by row, as `text` reads them, for a copy that keeps their colours too.
    pub fn pieces(&self, grid: &Grid) -> Option<Vec<Piece>> {
        let (first, last) = self.range(grid)?;
        let mut pieces = Vec::new();
        for line in first.line..=last.line {
            let Some(row) = grid.row_global(line) else { continue };
            let end = row.cells.len() - 1;
            let (from, to) = match self.kind {
                Kind::Block => (first.column, last.column),
                _ => (
                    if line == first.line { first.column } else { 0 },
                    if line == last.line { last.column } else { end },
                ),
            };
            let from = if row
                .cells
                .get(from)
                .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER))
            {
                from.saturating_sub(1)
            } else {
                from
            };
            let spacer = CellFlags(CellFlags::WIDE_SPACER.0 | CellFlags::LEADING_WIDE_SPACER.0);
            let columns = (from..=to.min(end))
                .filter(|&column| !row.cells[column].flags.intersects(spacer))
                .collect();
            let joined = row.wrapped() && to == end && self.kind != Kind::Block;
            pieces.push(Piece {
                line,
                columns,
                joined,
                newline: !joined && (line != last.line || self.kind == Kind::Line),
            });
        }
        Some(pieces)
    }
}

/// The cells a selection takes in one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The row's global line number.
    pub line: u64,
    /// The cells that hold a character, left to right; the second halves of wide characters are left out.
    pub columns: Vec<usize>,
    /// The row wrapped into the next one, so the text goes on there, blanks and all.
    pub joined: bool,
    /// A newline follows the row's text, once its trailing blanks are dropped.
    pub newline: bool,
}

/// The corners of a block between `a` and `b`, `a` on the upper line; a side leaves out the column it points away
/// from, as in a simple selection.
fn block(a: (Anchor, Side), b: (Anchor, Side)) -> Option<(Anchor, Anchor)> {
    let (left, right) = if b.0.column < a.0.column { (b, a) } else { (a, b) };
    if left.0.column == right.0.column && left.1 == right.1 {
        return None;
    }
    let (mut from, mut to) = (left.0.column, right.0.column);
    if left.0 != right.0 {
        if right.1 == Side::Left {
            to = to.checked_sub(1)?;
        }
        if left.1 == Side::Right {
            from += 1;
        }
    }
    (from <= to).then_some((
        Anchor {
            line: a.0.line,
            column: from,
        },
        Anchor {
            line: b.0.line,
            column: to,
        },
    ))
}

fn is_word_end(grid: &Grid, at: Anchor) -> bool {
    grid.row_global(at.line)
        .and_then(|row| row.cells.get(at.column).copied())
        .is_none_or(|cell| WORD_ENDS.contains(cell.c))
}

fn word_start(grid: &Grid, mut at: Anchor) -> Anchor {
    if is_word_end(grid, at) {
        return at;
    }
    loop {
        let previous = if at.column > 0 {
            Anchor {
                line: at.line,
                column: at.column - 1,
            }
        } else {
            let wrapped = at.line > 0 && grid.row_global(at.line - 1).is_some_and(|row| row.wrapped());
            if !wrapped {
                return at;
            }
            Anchor {
                line: at.line - 1,
                column: grid.columns() - 1,
            }
        };
        if is_word_end(grid, previous) {
            return at;
        }
        at = previous;
    }
}

fn word_end(grid: &Grid, mut at: Anchor) -> Anchor {
    if is_word_end(grid, at) {
        return at;
    }
    loop {
        let wraps = grid.row_global(at.line).is_some_and(|row| row.wrapped());
        let next = if at.column + 1 < grid.columns() {
            Anchor {
                line: at.line,
                column: at.column + 1,
            }
        } else if wraps {
            Anchor {
                line: at.line + 1,
                column: 0,
            }
        } else {
            return at;
        };
        if is_word_end(grid, next) {
            return at;
        }
        at = next;
    }
}
