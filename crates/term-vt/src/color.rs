//! Colours as programs name them: one of the 16 named colours or the defaults, an entry of the 256-colour table, or
//! red, green and blue; the theme resolves them when a frame is drawn.

/// A colour of a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Color {
    /// A named colour: 0 to 15 the ANSI colours, or one of the defaults below.
    Named(u16),
    /// An entry of the 256-colour table.
    Indexed(u8),
    /// A colour given as red, green and blue.
    Rgb(u8, u8, u8),
}

impl Color {
    /// The default text colour.
    pub const FOREGROUND: Color = Color::Named(256);
    /// The default background.
    pub const BACKGROUND: Color = Color::Named(257);

    /// The colour as the reference format writes it: `fg`, `bg`, a number, or `#rrggbb`.
    pub fn reference(self) -> String {
        match self {
            Color::Named(256) => "fg".into(),
            Color::Named(257) => "bg".into(),
            Color::Named(index) => index.to_string(),
            Color::Indexed(index) => index.to_string(),
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }
}
