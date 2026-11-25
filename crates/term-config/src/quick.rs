//! The quick terminal (F45): the edge of the screen it slides in from and how much of the screen it takes; the
//! global hotkey is whichever chord the key bindings give `quick_terminal`.

use serde::{Deserialize, Serialize};

/// The quick terminal's place and size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickTerminal {
    /// The edge of the active screen it slides in from.
    pub position: QuickPosition,
    /// How far it reaches into the screen from that edge, as a share of the screen's height or width.
    pub size: f64,
    /// How much of that edge it covers, as a share, centred on the edge.
    pub span: f64,
    /// Slides away as soon as another window or app takes the keyboard.
    pub autohide: bool,
}

impl Default for QuickTerminal {
    fn default() -> QuickTerminal {
        QuickTerminal {
            position: QuickPosition::Top,
            size: 0.4,
            span: 1.0,
            autohide: true,
        }
    }
}

impl QuickTerminal {
    /// `size` and `span` kept within what still shows a usable terminal and fits the screen.
    pub fn shares(&self) -> (f64, f64) {
        let clamp = |share: f64, low: f64| if share.is_finite() { share.clamp(low, 1.0) } else { 1.0 };
        (clamp(self.size, 0.1), clamp(self.span, 0.2))
    }
}

/// The edge the quick terminal slides in from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuickPosition {
    /// Down from the top, below the menu bar.
    Top,
    /// Up from the bottom.
    Bottom,
    /// In from the left.
    Left,
    /// In from the right.
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_stay_on_the_screen() {
        let quick = QuickTerminal {
            size: 3.0,
            span: f64::NAN,
            ..QuickTerminal::default()
        };
        assert_eq!(quick.shares(), (1.0, 1.0));
        let tiny = QuickTerminal {
            size: 0.0,
            span: -1.0,
            ..QuickTerminal::default()
        };
        assert_eq!(tiny.shares(), (0.1, 0.2));
    }

    #[test]
    fn the_section_reads_from_toml() {
        let (config, warnings) =
            crate::Config::parse("version = 1\n[quick_terminal]\nposition = \"left\"\nsize = 0.5\nautohide = false\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.quick_terminal.position, QuickPosition::Left);
        assert_eq!(config.quick_terminal.size, 0.5);
        assert!(!config.quick_terminal.autohide);
        assert_eq!(config.quick_terminal.span, 1.0);
    }
}
