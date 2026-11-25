//! The terminal's colours as the renderer draws them, made from a theme of the settings (F162).

use term_config::theme::{Color, Theme as Configured};
use term_session::{Palette, Rgb};

/// The colours a terminal view draws with.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// What the session resolves cell colours from.
    pub palette: Palette,
    /// Behind selected text.
    pub selection: Rgb,
    /// The least contrast text keeps against its background (F164); 1 leaves the colours alone.
    pub minimum_contrast: f64,
}

fn rgb(color: Color) -> Rgb {
    Rgb::new(color.r, color.g, color.b)
}

impl Theme {
    /// The colours of `theme`, with text kept at `minimum_contrast` at least.
    pub fn from_config(theme: &Configured, minimum_contrast: f64) -> Theme {
        Theme {
            palette: Palette {
                foreground: rgb(theme.foreground),
                background: rgb(theme.background),
                cursor: theme.cursor.map(rgb),
                ansi: theme.palette.map(rgb),
                bold_is_bright: true,
            },
            selection: rgb(theme.selection_background),
            minimum_contrast,
        }
    }

    /// `foreground` as it is drawn on `background`: moved just far enough to keep the minimum contrast.
    pub fn readable(&self, foreground: Rgb, background: Rgb) -> Rgb {
        if self.minimum_contrast <= 1.0 {
            return foreground;
        }
        let color = |c: Rgb| Color { r: c.r, g: c.g, b: c.b };
        rgb(term_config::theme::readable(
            color(foreground),
            color(background),
            self.minimum_contrast,
        ))
    }
}

/// The built-in colours for the dark or the light appearance; `high_contrast` follows the system's Increase Contrast.
pub fn theme(dark: bool, high_contrast: bool) -> Theme {
    Theme::from_config(&Configured::builtin(dark, high_contrast), 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimum_contrast_lifts_dim_text_on_its_background() {
        let mut theme = theme(true, false);
        let dim = Rgb::new(0x30, 0x30, 0x30);
        let ground = theme.palette.background;
        assert_eq!(theme.readable(dim, ground), dim);
        theme.minimum_contrast = 4.5;
        let lifted = theme.readable(dim, ground);
        assert!(lifted.r > dim.r && lifted.g > dim.g);
    }
}
