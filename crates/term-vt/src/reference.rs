//! The core's screen in the neutral format of the reference tests (E11), the one the expectations taken over from
//! Alacritty are written in (`tests/ref/README.md`).

use std::borrow::Cow;
use std::fmt::Write as _;

use crate::cell::CellFlags;
use crate::color::Color;
use crate::style::Attrs;
use crate::term::{Size, Terminal};

const ATTRS: [(Attrs, &str); 11] = [
    (Attrs::BOLD, "bold"),
    (Attrs::ITALIC, "italic"),
    (Attrs::DIM, "dim"),
    (Attrs::UNDERLINE, "underline"),
    (Attrs::DOUBLE_UNDERLINE, "double-underline"),
    (Attrs::UNDERCURL, "undercurl"),
    (Attrs::DOTTED_UNDERLINE, "dotted-underline"),
    (Attrs::DASHED_UNDERLINE, "dashed-underline"),
    (Attrs::STRIKE, "strike"),
    (Attrs::INVERSE, "inverse"),
    (Attrs::HIDDEN, "hidden"),
];

/// What the core ends up with after `recording` on a terminal of `size` keeping `history` lines, in the neutral
/// format.
pub fn replay(recording: &[u8], size: Size, history: usize) -> String {
    let mut terminal = Terminal::new(size, history);
    terminal.advance(recording);
    dump(&terminal, history)
}

/// What the core ends up with after each recording at the size beside it, resizing in between, in the neutral
/// format.
pub fn replay_resizing(steps: &[(Vec<u8>, Size)], history: usize) -> String {
    let Some((_, first)) = steps.first() else {
        return String::new();
    };
    let mut terminal = Terminal::new(*first, history);
    for (bytes, size) in steps {
        terminal.resize(*size);
        terminal.advance(bytes);
    }
    dump(&terminal, history)
}

/// The screen and history of `terminal` in the neutral format.
pub fn dump(terminal: &Terminal, history: usize) -> String {
    let size = terminal.size();
    let grid = terminal.grid();
    let cursor = terminal.cursor();
    let mut out = String::new();
    let _ = writeln!(out, "size {} {} {}", size.columns, size.lines, history);
    let _ = writeln!(out, "cursor {} {}", cursor.line, cursor.column);
    let mut cells = String::new();
    for (index, row) in grid
        .history
        .iter()
        .chain(grid.rows.iter().map(Cow::Borrowed))
        .enumerate()
    {
        let _ = writeln!(out, "line {index} {}", row.text().trim_end());
        for (column, cell) in row.cells.iter().enumerate() {
            let style = terminal.style(cell.style);
            let mut attributes = Vec::new();
            if style.fg != Color::FOREGROUND {
                attributes.push(format!("fg={}", style.fg.reference()));
            }
            if style.bg != Color::BACKGROUND {
                attributes.push(format!("bg={}", style.bg.reference()));
            }
            for (attr, name) in ATTRS {
                if style.attrs.contains(attr) {
                    attributes.push(name.to_string());
                }
            }
            let flags = [
                (CellFlags::WIDE, "wide"),
                (CellFlags::WIDE_SPACER, "wide-spacer"),
                (CellFlags::LEADING_WIDE_SPACER, "leading-spacer"),
                (CellFlags::WRAPPED, "wrapped"),
            ];
            for (flag, name) in flags {
                if cell.flags.contains(flag) {
                    attributes.push(name.to_string());
                }
            }
            if !attributes.is_empty() {
                let _ = writeln!(cells, "cell {index} {column} {}", attributes.join(" "));
            }
        }
    }
    out.push_str(&cells);
    out
}
