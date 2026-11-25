//! What the view draws: the visible cells with their final colours, the cursor, the selection and what changed.

use std::sync::Arc;

use crate::input::InputModes;

/// A colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[expect(missing_docs, reason = "the channels")]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// The colour from its channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// Two thirds as bright, as faint text is drawn.
    #[must_use]
    pub fn dim(self) -> Rgb {
        let third = |c: u8| u8::try_from(u16::from(c) * 2 / 3).unwrap_or(c);
        Rgb::new(third(self.r), third(self.g), third(self.b))
    }
}

/// The colours a session starts from; programs may change them (OSC 4, 10, 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// The default text colour.
    pub foreground: Rgb,
    /// The default background.
    pub background: Rgb,
    /// The cursor; `None` draws it in the text colour.
    pub cursor: Option<Rgb>,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then their bright forms.
    pub ansi: [Rgb; 16],
    /// Bold text in one of the first eight colours uses the bright form, as most terminals do.
    pub bold_is_bright: bool,
}

impl Default for Palette {
    fn default() -> Palette {
        Palette {
            foreground: Rgb::new(0xd8, 0xd8, 0xd8),
            background: Rgb::new(0x18, 0x18, 0x18),
            cursor: None,
            ansi: [
                Rgb::new(0x00, 0x00, 0x00),
                Rgb::new(0xcd, 0x31, 0x31),
                Rgb::new(0x0d, 0xbc, 0x79),
                Rgb::new(0xe5, 0xe5, 0x10),
                Rgb::new(0x24, 0x72, 0xc8),
                Rgb::new(0xbc, 0x3f, 0xbc),
                Rgb::new(0x11, 0xa8, 0xcd),
                Rgb::new(0xe5, 0xe5, 0xe5),
                Rgb::new(0x66, 0x66, 0x66),
                Rgb::new(0xf1, 0x4c, 0x4c),
                Rgb::new(0x23, 0xd1, 0x8b),
                Rgb::new(0xf5, 0xf5, 0x43),
                Rgb::new(0x3b, 0x8e, 0xea),
                Rgb::new(0xd6, 0x70, 0xd6),
                Rgb::new(0x29, 0xb8, 0xdb),
                Rgb::new(0xff, 0xff, 0xff),
            ],
            bold_is_bright: true,
        }
    }
}

impl Palette {
    /// The 256 colours of the table, then the text colour, background and cursor, as the own core answers questions
    /// about them.
    pub(crate) fn base_colors(&self) -> Vec<(u8, u8, u8)> {
        let rgb = |c: Rgb| (c.r, c.g, c.b);
        let mut colors: Vec<(u8, u8, u8)> = (0..=255u8).map(|i| rgb(self.indexed(i))).collect();
        colors.push(rgb(self.foreground));
        colors.push(rgb(self.background));
        colors.push(rgb(self.cursor.unwrap_or(self.foreground)));
        colors
    }

    /// Colour `index` of the 256-colour table: the ANSI colours, a 6×6×6 cube, then 24 greys.
    pub(crate) fn indexed(&self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let level = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
                let n = index - 16;
                Rgb::new(level(n / 36), level(n / 6 % 6), level(n % 6))
            }
            _ => {
                let grey = 8 + 10 * (index - 232);
                Rgb::new(grey, grey, grey)
            }
        }
    }
}

/// How a cell is drawn besides its colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags(u16);

#[expect(missing_docs, reason = "the names are the attributes")]
impl CellFlags {
    pub const BOLD: CellFlags = CellFlags(1);
    pub const ITALIC: CellFlags = CellFlags(1 << 1);
    pub(crate) const DIM: CellFlags = CellFlags(1 << 2);
    pub const UNDERLINE: CellFlags = CellFlags(1 << 3);
    pub const DOUBLE_UNDERLINE: CellFlags = CellFlags(1 << 4);
    pub const UNDERCURL: CellFlags = CellFlags(1 << 5);
    pub const DOTTED_UNDERLINE: CellFlags = CellFlags(1 << 6);
    pub const DASHED_UNDERLINE: CellFlags = CellFlags(1 << 7);
    pub const STRIKETHROUGH: CellFlags = CellFlags(1 << 8);
    /// Foreground and background are swapped; `Cell::fg` and `Cell::bg` already are.
    pub(crate) const INVERSE: CellFlags = CellFlags(1 << 9);
    /// The text is not drawn.
    pub const HIDDEN: CellFlags = CellFlags(1 << 10);
    /// A double-width character, drawn across this cell and the next.
    pub const WIDE: CellFlags = CellFlags(1 << 11);
    /// The second half of a double-width character; nothing to draw.
    pub const WIDE_SPACER: CellFlags = CellFlags(1 << 12);
    /// The line continues on the next row because it wrapped here.
    pub const WRAPPED: CellFlags = CellFlags(1 << 13);
}

impl CellFlags {
    /// Sets `flags`.
    pub(crate) fn insert(&mut self, flags: CellFlags) {
        self.0 |= flags.0;
    }

    /// Whether all of `flags` are set.
    pub fn contains(self, flags: CellFlags) -> bool {
        self.0 & flags.0 == flags.0
    }
}

/// One cell of the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// The character; a space when empty.
    pub ch: char,
    /// Combining characters drawn on top of it.
    pub zero_width: Vec<char>,
    /// The text colour, inverse and faint already applied.
    pub fg: Rgb,
    /// The background, inverse already applied.
    pub bg: Rgb,
    /// A colour of its own for the underline (SGR 58).
    pub underline_color: Option<Rgb>,
    /// Attributes.
    pub flags: CellFlags,
    /// The target of an OSC 8 hyperlink over this cell, shared by the cells next to it that the same link covers.
    pub hyperlink: Option<Arc<str>>,
}

/// A cell position on the visible screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Point {
    /// The line, from 0 at the top.
    pub line: usize,
    /// The column, from 0 at the left.
    pub column: usize,
}

/// The cursor's look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names are the shapes")]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    /// A frame, as for a terminal without focus.
    HollowBlock,
}

/// The cursor as drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Cursor {
    /// Where it is.
    pub point: Point,
    pub shape: CursorShape,
    /// Whether to draw it: the program has not hidden it and it is not scrolled out of view.
    pub visible: bool,
    /// The program asked for a blinking cursor.
    pub blinking: bool,
    /// It stands on a double-width character.
    pub wide: bool,
}

/// The selected part of the visible screen, from `start` to `end` inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Highlight {
    /// The first selected cell.
    pub start: Point,
    /// The last selected cell.
    pub end: Point,
    /// Only the columns between `start` and `end` on each line, as in a block selection.
    pub block: bool,
}

/// What changed since the previous snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Damage {
    /// Draw everything.
    Full,
    /// Draw these parts of lines; the rest is unchanged.
    Lines(Vec<LineDamage>),
}

/// The changed columns of a line, `left` to `right` inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the struct says it")]
pub struct LineDamage {
    pub line: usize,
    pub(crate) left: usize,
    pub(crate) right: usize,
}

/// The visible screen at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Snapshot {
    /// Columns per line.
    pub columns: usize,
    /// Visible lines.
    pub lines: usize,
    /// The visible lines, top first, each `columns` cells long.
    pub rows: Vec<Vec<Cell>>,
    pub cursor: Cursor,
    /// The selection where it is visible.
    pub selection: Option<Highlight>,
    /// How many lines the view is scrolled back into the history.
    pub display_offset: usize,
    /// How many lines the history holds.
    pub history: usize,
    /// What changed since the previous snapshot.
    pub damage: Damage,
    /// The background the screen is cleared to.
    pub background: Rgb,
    /// The input modes, such as whether the program takes the mouse.
    pub modes: InputModes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_256_colour_table_has_the_cube_and_the_grey_ramp() {
        let palette = Palette::default();
        assert_eq!(palette.indexed(1), palette.ansi[1]);
        assert_eq!(palette.indexed(16), Rgb::new(0, 0, 0));
        assert_eq!(palette.indexed(21), Rgb::new(0, 0, 255));
        assert_eq!(palette.indexed(196), Rgb::new(255, 0, 0));
        assert_eq!(palette.indexed(231), Rgb::new(255, 255, 255));
        assert_eq!(palette.indexed(232), Rgb::new(8, 8, 8));
        assert_eq!(palette.indexed(255), Rgb::new(238, 238, 238));
    }
}
