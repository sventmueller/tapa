//! The terminal's settings (E14): one TOML file with a version number, migrated when an older version is read, which
//! the settings window writes too and the app reloads while it runs (F159, F160). Profiles inherit from one another
//! (F161), themes come in a light and a dark variant (F162), and key bindings report their conflicts (F167).
//!
//! ```
//! let (config, warnings) = term_config::Config::parse("version = 1\n[font]\nsize = 15\n");
//! assert!(warnings.is_empty());
//! assert_eq!(config.font.size, 15.0);
//! assert_eq!(config.font.family, term_config::Font::default().family);
//! ```

mod agent;
mod browser;
mod dictation;
mod edit;
mod editor;
mod keys;
mod migrate;
mod profile;
mod quick;
mod restore;
mod schema;
pub mod theme;
mod watch;

pub use agent::{Agent, Claude, ClaudeMode, Codex, CodexApproval, OpenCode};
pub use browser::{Browser, SearchEngine};
pub use dictation::Dictation;
pub use edit::{Document, EditError, Origin, Part, chords_text, number, parse_chords, strings, unique_profile_name};
pub use editor::{Editor, Files};
pub use keys::{Action, Binding, Chord, KeyError, conflicts, default_bindings};
pub use profile::{Profile, ProfileError};
pub use quick::{QuickPosition, QuickTerminal};
pub use restore::{MAX_RESTORED_LINES, Restore};
pub use schema::{
    Appearance, Config, Cursor, CursorStyle, Font, Inherit, Input, OptionAsMeta, Terminal, ThemeChoice, VERSION, Window,
};
pub use watch::{Watcher, watch};

use std::io;
use std::path::{Path, PathBuf};

/// Where the settings live unless the user chose another file: `config.toml` in the data folder.
pub fn default_path() -> PathBuf {
    term_base::data_dir().join("config.toml")
}

/// Something in the file that was not understood, which the settings window shows; the rest still applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning(pub String);

impl Config {
    /// The settings in `text`, migrated to the current version; what could not be read keeps its default and comes
    /// back as a warning, so one typo never costs the user all their settings.
    pub fn parse(text: &str) -> (Config, Vec<Warning>) {
        let mut warnings = Vec::new();
        let table = match text.parse::<toml::Table>() {
            Ok(table) => table,
            Err(error) => {
                warnings.push(Warning(error.to_string()));
                return (Config::default(), warnings);
            }
        };
        let table = migrate::to_current(table, &mut warnings);
        (schema::read(table, &mut warnings), warnings)
    }

    /// The settings in `path`; the defaults when the file does not exist yet.
    pub fn load(path: &Path) -> io::Result<(Config, Vec<Warning>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Config::parse(&text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((Config::default(), Vec::new())),
            Err(error) => Err(error),
        }
    }

    /// The settings as TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Writes the settings to `path` atomically, so a reader never sees half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        term_base::atomic_write::write(path, self.to_toml().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_read_back_the_same() {
        let dir = term_base::testing::scratch("config-roundtrip");
        let path = dir.join("config.toml");
        let mut config = Config::default();
        config.font.size = 17.0;
        config.cursor.style = CursorStyle::Bar;
        config.window.sidebar = false;
        config.save(&path).unwrap();
        let (read, warnings) = Config::load(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(read, config);
    }

    #[test]
    fn the_interface_size_stands_beside_the_other_sizes() {
        let (config, warnings) = Config::parse("version = 1\n[editor]\nfont_size = 15\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.editor.font_size, 15.0, "an older file keeps its editors' size");
        assert_eq!(config.window.interface_font_size, 13.0);
        let (config, warnings) = Config::parse(
            "version = 1\n[window]\ninterface_font_size = 11\n[editor]\nfont_size = 16\n[font]\nsize = 14\n[agent]\nfont_size = 17\n",
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            (
                config.window.interface_font_size,
                config.editor.font_size,
                config.font.size,
                config.agent.font_size
            ),
            (11.0, 16.0, 14.0, 17.0)
        );
    }

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let dir = term_base::testing::scratch("config-missing");
        let (config, warnings) = Config::load(&dir.join("none.toml")).unwrap();
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_broken_file_keeps_the_defaults_and_says_why() {
        let (config, warnings) = Config::parse("font = [");
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_wrong_value_costs_only_its_own_section() {
        let (config, warnings) = Config::parse("version = 1\n[font]\nsize = \"big\"\n[cursor]\nstyle = \"bar\"\n");
        assert_eq!(config.font, Font::default());
        assert_eq!(config.cursor.style, CursorStyle::Bar);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].0.contains("font"), "{warnings:?}");
    }
}
