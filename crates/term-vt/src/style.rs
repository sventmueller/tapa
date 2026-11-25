//! How cells look besides their text, kept once in a table and referred to by number from each cell (E4), so a cell
//! stays small however many colours and attributes a program uses.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::color::Color;

/// The attributes of a style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Attrs(pub u16);

#[expect(missing_docs, reason = "the names are the attributes")]
impl Attrs {
    pub const BOLD: Attrs = Attrs(1);
    pub const DIM: Attrs = Attrs(1 << 1);
    pub const ITALIC: Attrs = Attrs(1 << 2);
    pub const UNDERLINE: Attrs = Attrs(1 << 3);
    pub const DOUBLE_UNDERLINE: Attrs = Attrs(1 << 4);
    pub const UNDERCURL: Attrs = Attrs(1 << 5);
    pub const DOTTED_UNDERLINE: Attrs = Attrs(1 << 6);
    pub const DASHED_UNDERLINE: Attrs = Attrs(1 << 7);
    pub const INVERSE: Attrs = Attrs(1 << 8);
    pub const HIDDEN: Attrs = Attrs(1 << 9);
    pub const STRIKE: Attrs = Attrs(1 << 10);
    pub const BLINK: Attrs = Attrs(1 << 11);
    /// Every kind of underline, of which a cell has one at most.
    pub const UNDERLINES: Attrs = Attrs((1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7));
}

impl Attrs {
    /// Whether all of `other` are set.
    pub fn contains(self, other: Attrs) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets `other`.
    pub fn insert(&mut self, other: Attrs) {
        self.0 |= other.0;
    }

    /// Clears `other`.
    pub fn remove(&mut self, other: Attrs) {
        self.0 &= !other.0;
    }
}

/// How a cell looks besides its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    /// The text colour.
    pub fg: Color,
    /// The background.
    pub bg: Color,
    /// The underline's colour; none for the text colour.
    pub underline: Option<Color>,
    /// Bold, italic and the rest.
    pub attrs: Attrs,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            fg: Color::FOREGROUND,
            bg: Color::BACKGROUND,
            underline: None,
            attrs: Attrs::default(),
        }
    }
}

impl Style {
    /// The style of a cell cleared to background `bg`: default colours and no attributes over it.
    pub fn blank(bg: Color) -> Style {
        Style { bg, ..Style::default() }
    }
}

/// A style's number in its table; 0 is the default style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct StyleId(pub u16);

/// The styles of a terminal.
#[derive(Debug, Clone)]
pub struct Styles {
    styles: Vec<Style>,
    ids: HashMap<Style, StyleId, BuildHasherDefault<Quick>>,
}

/// FxHash's rotate and multiply for the style table, instead of SipHash: a program that sends crafted styles to collide
/// gains nothing it could not already cause by sending many styles, as the table is bounded and compacted.
#[derive(Default)]
struct Quick(u64);

impl Hasher for Quick {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(u64::from(byte));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

impl Default for Styles {
    fn default() -> Styles {
        let default = Style::default();
        Styles {
            styles: vec![default],
            ids: HashMap::from_iter([(default, StyleId(0))]),
        }
    }
}

impl Styles {
    /// The number of `style`, which is added if it is new; `None` once the table is full and wants compacting.
    pub fn intern(&mut self, style: Style) -> Option<StyleId> {
        if let Some(&id) = self.ids.get(&style) {
            return Some(id);
        }
        let id = StyleId(u16::try_from(self.styles.len()).ok()?);
        self.styles.push(style);
        self.ids.insert(style, id);
        Some(id)
    }

    /// The style numbered `id`.
    pub fn get(&self, id: StyleId) -> Style {
        self.styles.get(usize::from(id.0)).copied().unwrap_or_default()
    }

    /// How many styles there are.
    pub fn len(&self) -> usize {
        self.styles.len()
    }

    /// Whether only the default style is there, which it always is.
    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// A table of only the styles `used` marks, and for every old number its new one.
    pub fn compact(&self, used: &[bool]) -> (Styles, Vec<StyleId>) {
        let mut table = Styles::default();
        let mut map = vec![StyleId(0); self.styles.len()];
        for (index, style) in self.styles.iter().enumerate() {
            if used.get(index).copied().unwrap_or(false) || index == 0 {
                map[index] = table.intern(*style).unwrap_or_default();
            }
        }
        (table, map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_style_is_kept_once_and_compacting_keeps_the_used_ones() {
        let mut styles = Styles::default();
        let red = Style {
            fg: Color::Named(1),
            ..Style::default()
        };
        let blue = Style {
            fg: Color::Named(4),
            ..Style::default()
        };
        let a = styles.intern(red).unwrap();
        assert_eq!(styles.intern(red), Some(a));
        let b = styles.intern(blue).unwrap();
        assert_eq!(styles.len(), 3);
        let mut used = vec![false; styles.len()];
        used[usize::from(b.0)] = true;
        let (compacted, map) = styles.compact(&used);
        assert_eq!(compacted.len(), 2);
        assert_eq!(compacted.get(map[usize::from(b.0)]), blue);
    }
}
