//! Fonts and the colours of code, which follow Light, Dark and Increase Contrast by themselves.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSFontManager, NSFontTraitMask, NSFontWeightBold, NSFontWeightRegular};
use splat_markdown::code::CodeToken;
use splat_markdown::highlight::Token;
use splat_markdown::palette;
use term_ui::kit::colour::{Appearance, Rgb, dynamic};

pub(crate) use term_ui::kit::style::mono as code_font;

use crate::util::constant;

thread_local! {
    static TOKEN_COLORS: RefCell<HashMap<Token, Retained<NSColor>>> = RefCell::new(HashMap::new());
    static STYLED_FONTS: RefCell<HashMap<(u64, bool, bool), Retained<NSFont>>> = RefCell::new(HashMap::new());
}

/// The code font at `size`, bold and italic as Markdown's source asks; the same width as the plain one.
pub(crate) fn styled_code_font(size: f64, bold: bool, italic: bool) -> Retained<NSFont> {
    STYLED_FONTS.with(|cache| {
        cache
            .borrow_mut()
            .entry((size.to_bits(), bold, italic))
            .or_insert_with(|| {
                let weight = if bold {
                    constant!(NSFontWeightBold)
                } else {
                    constant!(NSFontWeightRegular)
                };
                let font = NSFont::monospacedSystemFontOfSize_weight(size, weight);
                match objc2::MainThreadMarker::new().filter(|_| italic) {
                    Some(mtm) => NSFontManager::sharedFontManager(mtm)
                        .convertFont_toHaveTrait(&font, NSFontTraitMask::ItalicFontMask),
                    None => font,
                }
            })
            .clone()
    })
}

/// The colour of a code token in the current appearance, splat's palette.
pub(crate) fn palette_color(token: Token) -> Retained<NSColor> {
    TOKEN_COLORS.with(|cache| {
        cache
            .borrow_mut()
            .entry(token)
            .or_insert_with(|| {
                dynamic(move |appearance| {
                    let appearance = match appearance {
                        Appearance::Light => palette::Appearance::Light,
                        Appearance::Dark => palette::Appearance::Dark,
                        Appearance::LightHighContrast => palette::Appearance::LightHighContrast,
                        Appearance::DarkHighContrast => palette::Appearance::DarkHighContrast,
                    };
                    let palette::Rgb(r, g, b) = palette::color(token, appearance);
                    Rgb(r, g, b)
                })
            })
            .clone()
    })
}

/// The colour of `token`, resolved per appearance with splat's palette (7:1 contrast).
pub(crate) fn token_color(token: CodeToken) -> Retained<NSColor> {
    palette_color(Token::from(token))
}
