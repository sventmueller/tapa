//! The sizes, fonts and colours of `term_ui::kit::style`, and the colours of code tokens from Splat's palette.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2_app_kit::NSColor;
use splat_markdown::highlight::Token;
use splat_markdown::palette;
use term_ui::kit::colour::dynamic;

pub use term_ui::kit::colour::{Appearance, Rgb};
pub use term_ui::kit::style::*;

thread_local! {
    static TOKEN_COLORS: RefCell<HashMap<Token, Retained<NSColor>>> = RefCell::new(HashMap::new());
}

/// Splat's palette for `appearance`.
fn splat_appearance(appearance: Appearance) -> palette::Appearance {
    match appearance {
        Appearance::Light => palette::Appearance::Light,
        Appearance::Dark => palette::Appearance::Dark,
        Appearance::LightHighContrast => palette::Appearance::LightHighContrast,
        Appearance::DarkHighContrast => palette::Appearance::DarkHighContrast,
    }
}

/// The colour of `token` in `appearance`, from Splat's palette.
fn token_rgb(token: Token, appearance: Appearance) -> Rgb {
    let palette::Rgb(r, g, b) = palette::color(token, splat_appearance(appearance));
    Rgb(r, g, b)
}

/// The colour of a code token in the current appearance, Splat's palette.
pub fn token_color(token: Token) -> Retained<NSColor> {
    TOKEN_COLORS.with(|cache| {
        cache
            .borrow_mut()
            .entry(token)
            .or_insert_with(|| dynamic(move |appearance| token_rgb(token, appearance)))
            .clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_has_splats_colour_in_every_appearance() {
        for appearance in [
            Appearance::Light,
            Appearance::Dark,
            Appearance::LightHighContrast,
            Appearance::DarkHighContrast,
        ] {
            let palette::Rgb(r, g, b) = palette::color(Token::Keyword, splat_appearance(appearance));
            assert_eq!(token_rgb(Token::Keyword, appearance), Rgb(r, g, b));
        }
        assert_ne!(
            token_rgb(Token::Keyword, Appearance::Light),
            token_rgb(Token::Keyword, Appearance::Dark)
        );
    }
}
