//! What comes back after quitting and starting again (F71): windows, tabs, panes, their folders and titles, and if
//! wanted the last lines of each session's history, stored encrypted (E17).

use serde::{Deserialize, Serialize};

/// The most lines of history kept per session, whatever the settings say.
pub const MAX_RESTORED_LINES: usize = 10_000;

/// What is restored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Restore {
    /// Reopens the windows, tabs and panes of the last time; off, nothing is stored.
    pub enabled: bool,
    /// Keeps the last lines of each session's history with them.
    pub scrollback: bool,
    /// How many lines of history per session, at most `MAX_RESTORED_LINES`.
    pub scrollback_lines: usize,
}

impl Default for Restore {
    fn default() -> Restore {
        Restore {
            enabled: true,
            scrollback: true,
            scrollback_lines: 1_000,
        }
    }
}

impl Restore {
    /// How many lines of history per session to keep: none when the settings say so.
    pub fn lines(&self) -> usize {
        if self.enabled && self.scrollback {
            self.scrollback_lines.min(MAX_RESTORED_LINES)
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_kept_are_capped_and_off_when_turned_off() {
        let (config, warnings) = crate::Config::parse("version = 1\n[restore]\nscrollback_lines = 50000\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.restore.lines(), MAX_RESTORED_LINES);
        let off = Restore {
            scrollback: false,
            ..Restore::default()
        };
        assert_eq!(off.lines(), 0);
        let disabled = Restore {
            enabled: false,
            ..Restore::default()
        };
        assert_eq!(disabled.lines(), 0);
    }
}
