//! Themes (F162, F164): the terminal's colours in Ghostty's theme format, so the iTerm2-Color-Schemes collection reads
//! as it is; two built in, the rest from the app's resources and the user's themes folder, and a floor for the
//! contrast of text against its background.

use std::collections::BTreeMap;
use std::path::Path;

/// A colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Color {
    /// The colour `#rrggbb`.
    pub const fn hex(rgb: u32) -> Color {
        let [_, r, g, b] = rgb.to_be_bytes();
        Color { r, g, b }
    }

    /// The colour written as `#rrggbb`, `rrggbb` or `#rgb`.
    pub fn parse(text: &str) -> Option<Color> {
        let digits = text.trim().trim_start_matches('#');
        let value = u32::from_str_radix(digits, 16).ok()?;
        match digits.len() {
            6 => Some(Color::hex(value)),
            3 => {
                let [_, _, high, low] = value.to_be_bytes();
                let spread = |n: u8| n * 17;
                Some(Color {
                    r: spread(high & 0xf),
                    g: spread(low >> 4),
                    b: spread(low & 0xf),
                })
            }
            _ => None,
        }
    }

    /// The relative luminance WCAG defines.
    pub fn luminance(self) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// The contrast ratio of two colours as WCAG defines it, from 1 to 21.
    pub fn contrast(self, other: Color) -> f64 {
        let (x, y) = (self.luminance() + 0.05, other.luminance() + 0.05);
        x.max(y) / x.min(y)
    }

    fn mix(self, other: Color, amount: f64) -> Color {
        let channel = |a: u8, b: u8| {
            let mixed = f64::from(a) + (f64::from(b) - f64::from(a)) * amount;
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "clamped to a byte"
            )]
            let byte = mixed.round().clamp(0.0, 255.0) as u8;
            byte
        };
        Color {
            r: channel(self.r, other.r),
            g: channel(self.g, other.g),
            b: channel(self.b, other.b),
        }
    }
}

/// `foreground`, moved towards black or white just as far as it takes to reach `minimum` contrast against
/// `background` (F164); unchanged when it reaches it already or `minimum` is 1 or less.
pub fn readable(foreground: Color, background: Color, minimum: f64) -> Color {
    if minimum <= 1.0 || foreground.contrast(background) >= minimum {
        return foreground;
    }
    let toward = if background.luminance() > 0.5 {
        Color::hex(0x000000)
    } else {
        Color::hex(0xffffff)
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..16 {
        let middle = f64::midpoint(low, high);
        if foreground.mix(toward, middle).contrast(background) >= minimum {
            high = middle;
        } else {
            low = middle;
        }
    }
    foreground.mix(toward, high)
}

/// The colours of a theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Its name, the file name without extension.
    pub name: String,
    /// The default text colour.
    pub foreground: Color,
    /// The default background.
    pub background: Color,
    /// The cursor; `None` draws it in the text colour.
    pub cursor: Option<Color>,
    /// Text under a block cursor; `None` uses the background.
    pub cursor_text: Option<Color>,
    /// Behind selected text.
    pub selection_background: Color,
    /// Selected text; `None` keeps its own colour.
    pub selection_foreground: Option<Color>,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then their bright forms.
    pub palette: [Color; 16],
}

const DARK_ANSI: [u32; 16] = [
    0x3a3a3c, 0xf0716b, 0x8bcb6e, 0xe3c26a, 0x6aa7f5, 0xc98be6, 0x5cc6cf, 0xcfcfd2, 0x7f7f85, 0xff8e88, 0xa6e08c,
    0xf5d98a, 0x8dbeff, 0xdda6f5, 0x7ddce3, 0xffffff,
];

/// Darker than the usual terminal colours, so every one of them stays readable on white.
const LIGHT_ANSI: [u32; 16] = [
    0x1d1d1f, 0xc5302b, 0x2e7d32, 0x8f6a00, 0x1e5bc6, 0x9339b0, 0x0b7a86, 0xa9a9ae, 0x6e6e73, 0xe0463f, 0x3b9a45,
    0xb08400, 0x3478f6, 0xb157d1, 0x1597a6, 0xd5d5da,
];

impl Theme {
    /// The built-in theme for the dark or the light appearance; `high_contrast` follows the system's Increase
    /// Contrast.
    pub fn builtin(dark: bool, high_contrast: bool) -> Theme {
        let (foreground, background, ansi, selection) = match (dark, high_contrast) {
            (true, false) => (0xd6d6d8, 0x1e1e1e, DARK_ANSI, 0x3b5277),
            (true, true) => (0xffffff, 0x000000, DARK_ANSI, 0x2f5fa8),
            (false, false) => (0x1d1d1f, 0xffffff, LIGHT_ANSI, 0xb3d4fc),
            (false, true) => (0x000000, 0xffffff, LIGHT_ANSI, 0x8fbcf5),
        };
        Theme {
            name: if dark { "Tapa Dark" } else { "Tapa Light" }.into(),
            foreground: Color::hex(foreground),
            background: Color::hex(background),
            cursor: None,
            cursor_text: None,
            selection_background: Color::hex(selection),
            selection_foreground: None,
            palette: ansi.map(Color::hex),
        }
    }

    /// The theme in Ghostty's format, `key = value` per line; keys it does not know are skipped, and a theme without
    /// both colours of its own is none.
    pub fn parse(name: &str, text: &str) -> Option<Theme> {
        let mut theme = Theme::builtin(true, false);
        theme.name = name.to_string();
        let (mut foreground, mut background) = (false, false);
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "palette" => {
                    let Some((index, color)) = value.split_once('=') else {
                        continue;
                    };
                    if let (Ok(index), Some(color)) = (index.trim().parse::<usize>(), Color::parse(color))
                        && let Some(slot) = theme.palette.get_mut(index)
                    {
                        *slot = color;
                    }
                }
                "foreground" => {
                    if let Some(color) = Color::parse(value) {
                        theme.foreground = color;
                        foreground = true;
                    }
                }
                "background" => {
                    if let Some(color) = Color::parse(value) {
                        theme.background = color;
                        background = true;
                    }
                }
                "cursor-color" => theme.cursor = Color::parse(value),
                "cursor-text" => theme.cursor_text = Color::parse(value),
                "selection-background" => {
                    if let Some(color) = Color::parse(value) {
                        theme.selection_background = color;
                    }
                }
                "selection-foreground" => theme.selection_foreground = Color::parse(value),
                _ => {}
            }
        }
        (foreground && background).then_some(theme)
    }

    /// Whether the theme is meant for the dark appearance.
    pub fn is_dark(&self) -> bool {
        self.background.luminance() < 0.2
    }
}

/// The themes the app knows by name: the built-in ones and those in the folders, a later folder's theme replacing an
/// earlier one of the same name, so the user's own themes win over the app's.
#[derive(Debug, Clone, Default)]
pub struct Themes {
    by_name: BTreeMap<String, Theme>,
}

impl Themes {
    /// The built-in themes and every readable theme in `folders`.
    pub fn load(folders: &[&Path]) -> Themes {
        let mut themes = Themes::default();
        for theme in [Theme::builtin(false, false), Theme::builtin(true, false)] {
            themes.by_name.insert(theme.name.clone(), theme);
        }
        for folder in folders {
            let Ok(entries) = std::fs::read_dir(folder) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') {
                    continue;
                }
                let name = name.strip_suffix(".conf").unwrap_or(name);
                if let Some(theme) = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| Theme::parse(name, &text))
                {
                    themes.by_name.insert(theme.name.clone(), theme);
                }
            }
        }
        themes
    }

    /// The theme `name`, or the built-in one of the appearance when there is none of that name.
    pub fn get(&self, name: &str, dark: bool, high_contrast: bool) -> Theme {
        match self.by_name.get(name) {
            Some(theme) if !(high_contrast && theme.name.starts_with("Tapa ")) => theme.clone(),
            _ => Theme::builtin(dark, high_contrast),
        }
    }

    /// The names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// How many themes there are.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether there are none, which never happens with the built-in ones.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRACULA: &str = "palette = 0=#21222c\npalette = 1=#ff5555\nbackground = #282a36\nforeground = #f8f8f2\n\
                           cursor-color = #f8f8f2\nselection-background = #44475a\nunknown = 1\n";

    #[test]
    fn a_ghostty_theme_reads() {
        let theme = Theme::parse("Dracula", DRACULA).unwrap();
        assert_eq!(theme.background, Color::hex(0x282a36));
        assert_eq!(theme.palette[1], Color::hex(0xff5555));
        assert_eq!(theme.cursor, Some(Color::hex(0xf8f8f2)));
        assert!(theme.is_dark());
        assert!(Theme::parse("half", "background = #000000\n").is_none());
    }

    #[test]
    fn the_users_folder_wins_and_unknown_names_fall_back() {
        let dir = term_base::testing::scratch("themes");
        std::fs::write(dir.join("Dracula"), DRACULA).unwrap();
        std::fs::write(dir.join("Broken.conf"), "nothing").unwrap();
        let themes = Themes::load(&[&dir]);
        assert_eq!(themes.len(), 3);
        assert_eq!(themes.get("Dracula", true, false).name, "Dracula");
        assert_eq!(themes.get("Missing", false, false).name, "Tapa Light");
    }

    #[test]
    fn built_in_text_and_colours_stay_readable() {
        for dark in [false, true] {
            let theme = Theme::builtin(dark, false);
            assert!(theme.foreground.contrast(theme.background) > 7.0, "WCAG AAA");
            for index in (1..7).chain(9..15) {
                let ratio = theme.palette[index].contrast(theme.background);
                assert!(ratio > 3.0, "colour {index} in dark={dark}: {ratio:.2}");
            }
            assert!(theme.foreground.contrast(theme.selection_background) > 4.5);
        }
    }

    #[test]
    fn a_minimum_contrast_moves_text_just_far_enough() {
        let grey = Color::hex(0x777777);
        let black = Color::hex(0x000000);
        assert_eq!(readable(grey, black, 1.0), grey);
        let moved = readable(grey, black, 7.0);
        assert!(moved.contrast(black) >= 7.0);
        assert!(moved.contrast(black) < 7.3, "not further than needed");
        assert_eq!(readable(Color::hex(0xffffff), black, 7.0), Color::hex(0xffffff));
        assert_eq!(Color::parse("#fff"), Some(Color::hex(0xffffff)));
    }
}
