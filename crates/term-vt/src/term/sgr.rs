//! SGR: the attributes and colours of the characters that follow.

use crate::color::Color;
use crate::parser::Params;
use crate::style::{Attrs, Style};

/// `style` with the attributes of `params` applied, as `CSI … m` asks.
pub(super) fn apply(style: &mut Style, params: &Params) {
    if params.is_empty() {
        *style = Style::default();
        return;
    }
    let mut iter = params.iter();
    while let Some(param) = iter.next() {
        let set_underline = |style: &mut Style, kind: Attrs| {
            style.attrs.remove(Attrs::UNDERLINES);
            style.attrs.insert(kind);
        };
        match param {
            [0] => *style = Style::default(),
            [1] => style.attrs.insert(Attrs::BOLD),
            [2] => style.attrs.insert(Attrs::DIM),
            [3] => style.attrs.insert(Attrs::ITALIC),
            [4, 0] => style.attrs.remove(Attrs::UNDERLINES),
            [4, 2] => set_underline(style, Attrs::DOUBLE_UNDERLINE),
            [4, 3] => set_underline(style, Attrs::UNDERCURL),
            [4, 4] => set_underline(style, Attrs::DOTTED_UNDERLINE),
            [4, 5] => set_underline(style, Attrs::DASHED_UNDERLINE),
            [4, ..] => set_underline(style, Attrs::UNDERLINE),
            [5 | 6] => style.attrs.insert(Attrs::BLINK),
            [7] => style.attrs.insert(Attrs::INVERSE),
            [8] => style.attrs.insert(Attrs::HIDDEN),
            [9] => style.attrs.insert(Attrs::STRIKE),
            [21] => style.attrs.remove(Attrs::BOLD),
            [22] => style.attrs.remove(Attrs(Attrs::BOLD.0 | Attrs::DIM.0)),
            [23] => style.attrs.remove(Attrs::ITALIC),
            [24] => style.attrs.remove(Attrs::UNDERLINES),
            [25] => style.attrs.remove(Attrs::BLINK),
            [27] => style.attrs.remove(Attrs::INVERSE),
            [28] => style.attrs.remove(Attrs::HIDDEN),
            [29] => style.attrs.remove(Attrs::STRIKE),
            [n @ 30..=37] => style.fg = Color::Named(n - 30),
            [38] => {
                if let Some(color) = extended(&mut iter) {
                    style.fg = color;
                }
            }
            [38, rest @ ..] => {
                if let Some(color) = colon(rest) {
                    style.fg = color;
                }
            }
            [39] => style.fg = Color::FOREGROUND,
            [n @ 40..=47] => style.bg = Color::Named(n - 40),
            [48] => {
                if let Some(color) = extended(&mut iter) {
                    style.bg = color;
                }
            }
            [48, rest @ ..] => {
                if let Some(color) = colon(rest) {
                    style.bg = color;
                }
            }
            [49] => style.bg = Color::BACKGROUND,
            [58] => {
                if let Some(color) = extended(&mut iter) {
                    style.underline = Some(color);
                }
            }
            [58, rest @ ..] => {
                if let Some(color) = colon(rest) {
                    style.underline = Some(color);
                }
            }
            [59] => style.underline = None,
            [n @ 90..=97] => style.fg = Color::Named(n - 90 + 8),
            [n @ 100..=107] => style.bg = Color::Named(n - 100 + 8),
            _ => {}
        }
    }
}

/// A colour in the form `38;5;n` or `38;2;r;g;b`, read from the parameters after the 38.
fn extended<'a>(iter: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    let mut next = || iter.next().and_then(|values| values.first().copied());
    match next()? {
        2 => {
            let r = u8::try_from(next()?).ok()?;
            let g = u8::try_from(next()?).ok()?;
            let b = u8::try_from(next()?).ok()?;
            Some(Color::Rgb(r, g, b))
        }
        5 => Some(Color::Indexed(u8::try_from(next()?).ok()?)),
        _ => None,
    }
}

/// A colour in the form `38:5:n`, `38:2:r:g:b` or `38:2:space:r:g:b`.
fn colon(rest: &[u16]) -> Option<Color> {
    match rest {
        [5, n, ..] => Some(Color::Indexed(u8::try_from(*n).ok()?)),
        [2, _, r, g, b, ..] if rest.len() > 4 => Some(Color::Rgb(
            u8::try_from(*r).ok()?,
            u8::try_from(*g).ok()?,
            u8::try_from(*b).ok()?,
        )),
        [2, r, g, b] => Some(Color::Rgb(
            u8::try_from(*r).ok()?,
            u8::try_from(*g).ok()?,
            u8::try_from(*b).ok()?,
        )),
        _ => None,
    }
}
