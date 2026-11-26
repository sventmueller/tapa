//! The colours of the command typed at the shell's prompt without the input field (F92), as the input field draws
//! them: drawn over the shell's own text where the shell left it in the text's colour, so a shell that colours its line
//! itself keeps its colours.

use std::ops::Range;

use term_session::{Cell, CellFlags, Point, Rgb, Snapshot};

use crate::palette::Theme;
use crate::render::{Canvas, Face, Fonts, Overlay, Stroke, selected_columns};

/// What a part of the command is, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The program.
    Command,
    /// A subcommand its spec knows.
    Subcommand,
    /// An option.
    Option,
    /// A variable.
    Variable,
    /// An operator between commands, such as `|` or `&&`.
    Operator,
}

/// The colour of `kind` in `theme`, kept readable on its background.
pub fn colour(theme: &Theme, kind: Kind) -> Rgb {
    let index = match kind {
        Kind::Command => 2,
        Kind::Subcommand => 6,
        Kind::Option => 4,
        Kind::Variable => 5,
        Kind::Operator => 3,
    };
    ansi(theme, index)
}

/// The colour of the underline under a command the shell cannot run.
pub fn error(theme: &Theme) -> Rgb {
    ansi(theme, 1)
}

fn ansi(theme: &Theme, index: usize) -> Rgb {
    theme.readable(theme.palette.ansi[index], theme.palette.background)
}

/// One part of the typed text: the characters it covers, its colour's kind, and whether the shell cannot run it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The characters of the typed text, counted from its start.
    pub chars: Range<usize>,
    /// What colours it, where anything does.
    pub kind: Option<Kind>,
    /// The shell cannot run it, which a dashed red line under it says.
    pub unknown: bool,
}

/// The parts of the text typed between `start` and the cursor at `cursor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tint {
    /// Where the typed text starts.
    pub start: Point,
    /// Where the cursor was when the parts were found; they show only while it is still there.
    pub cursor: Point,
    /// The parts.
    pub spans: Vec<Span>,
}

/// The cells from `start` up to `cursor`, one per character as the typed text counts them.
fn cells(screen: &Snapshot, start: Point, cursor: Point) -> Vec<(Point, &Cell)> {
    let mut found = Vec::new();
    for line in start.line..=cursor.line {
        let Some(row) = screen.rows.get(line) else { break };
        let from = if line == start.line { start.column } else { 0 };
        let to = if line == cursor.line {
            cursor.column.min(row.len())
        } else {
            row.len()
        };
        for (column, cell) in row.iter().enumerate().take(to).skip(from) {
            if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                found.push((Point { line, column }, cell));
            }
        }
    }
    found
}

/// Draws the parts of `overlay`'s tint over the typed text, while the cursor is where they were found for.
pub(crate) fn paint(canvas: &mut impl Canvas, fonts: &Fonts, theme: &Theme, screen: &Snapshot, overlay: &Overlay) {
    let Some(tint) = overlay.tint else { return };
    if screen.display_offset != 0 || screen.cursor.point != tint.cursor || tint.start > tint.cursor {
        return;
    }
    let m = fonts.metrics;
    let cells = cells(screen, tint.start, tint.cursor);
    let selected = |point: Point| {
        selected_columns(screen, point.line).is_some_and(|(from, to)| (from..=to).contains(&point.column))
    };
    for span in &tint.spans {
        let covered = cells
            .get(span.chars.start.min(cells.len())..span.chars.end.min(cells.len()))
            .unwrap_or_default();
        if let Some(kind) = span.kind {
            for &(point, cell) in covered {
                if cell.fg != theme.palette.foreground || cell.flags.contains(CellFlags::HIDDEN) || selected(point) {
                    continue;
                }
                let area = m.cell_rect(point, width(cell));
                canvas.fill(cell.bg, 1.0, area);
                let mut text = String::from(cell.ch);
                text.extend(&cell.zero_width);
                let baseline = area.origin.y + m.baseline;
                canvas.line(
                    &text,
                    Face::of(cell.flags),
                    area.origin.x,
                    baseline,
                    colour(theme, kind),
                );
            }
        }
        if span.unknown {
            for run in covered.chunk_by(|a, b| a.0.line == b.0.line) {
                let (Some(&(first, _)), Some(&(last, cell))) = (run.first(), run.last()) else {
                    continue;
                };
                let area = m.cell_rect(first, last.column + width(cell) - first.column);
                let baseline = area.origin.y + m.baseline;
                canvas.underline(Stroke::Dashed, area, baseline + m.underline, error(theme));
            }
        }
    }
}

/// How many cells `cell`'s character takes.
fn width(cell: &Cell) -> usize {
    if cell.flags.contains(CellFlags::WIDE) { 2 } else { 1 }
}
