//! The selection's cells with the colours the snapshot gives them, for a copy with formatting (F114).

use term_vt::style::Attrs;

use super::Own;
use crate::session::{Look, StyledText};
use crate::snapshot::Palette;

impl Own {
    pub(crate) fn selected_styled(&self, palette: &Palette) -> Option<StyledText> {
        let selection = self.selection.as_ref()?;
        let grid = self.terminal.grid();
        let mut styled = StyledText {
            lines: vec![Vec::new()],
            foreground: self.colour(palette, term_vt::term::colors::FOREGROUND),
            background: self.colour(palette, term_vt::term::colors::BACKGROUND),
        };
        let mut text = String::new();
        for piece in selection.pieces(grid)? {
            let Some(row) = grid.row_global(piece.line) else {
                continue;
            };
            for &column in &piece.columns {
                let cell = &row.cells[column];
                let style = self.terminal.style(cell.style);
                let mut fg = self.foreground(palette, style.fg, style.attrs);
                let mut bg = self.background(palette, style.bg);
                if style.attrs.contains(Attrs::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                text.clear();
                text.push(if cell.c == '\t' { ' ' } else { cell.c });
                if let Some(extra) = row.extra(column) {
                    text.push_str(&extra.zero_width);
                }
                let look = Look {
                    fg,
                    bg,
                    bold: style.attrs.contains(Attrs::BOLD),
                    italic: style.attrs.contains(Attrs::ITALIC),
                    underline: style.attrs.0 & Attrs::UNDERLINES.0 != 0,
                    strike: style.attrs.contains(Attrs::STRIKE),
                };
                styled.push(&text, look);
            }
            if !piece.joined {
                styled.trim_line();
                if piece.newline {
                    styled.new_line();
                }
            }
        }
        Some(styled)
    }
}

#[cfg(test)]
mod tests {
    use term_vt::selection::{Anchor, Kind, Selection, Side};
    use term_vt::{Size, Terminal};

    use super::*;

    #[test]
    fn a_copy_keeps_each_runs_colour_and_drops_the_blanks_at_line_ends() {
        let mut terminal = Terminal::new(Size { columns: 12, lines: 3 }, 0);
        terminal.advance(b"ok \x1b[1;31mfail\x1b[0m   \r\nnext\x1b[7m!\x1b[0m");
        let mut own = Own::new(terminal);
        own.selection = Some(Selection {
            kind: Kind::Simple,
            start: (Anchor { line: 0, column: 0 }, Side::Left),
            end: (Anchor { line: 1, column: 11 }, Side::Right),
        });
        let palette = Palette::default();
        let styled = own.selected_styled(&palette).expect("a selection");
        assert_eq!(styled.plain(), "ok fail\nnext!");
        assert_eq!(styled.plain(), own.selected_text().unwrap_or_default());
        let first = &styled.lines[0];
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].text, "fail");
        assert!(first[1].look.bold);
        let red = palette.indexed(if palette.bold_is_bright { 9 } else { 1 });
        assert_eq!(first[1].look.fg, red);
        let inverse = &styled.lines[1][1];
        assert_eq!(
            (inverse.look.fg, inverse.look.bg),
            (styled.background, styled.foreground)
        );
    }
}
