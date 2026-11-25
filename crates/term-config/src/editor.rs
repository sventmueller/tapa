//! The editor tabs and the file list of a workspace's sidebar entry (E22).

use serde::{Deserialize, Serialize};

/// The editor tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// The size of the code font in points.
    pub font_size: f64,
    /// Whether editors take Vim's keys (E105).
    pub vim: bool,
    /// Whether Vim's unnamed register is the system pasteboard.
    pub vim_clipboard: bool,
    /// Whether spaces, tabs and line ends show.
    pub show_whitespace: bool,
    /// Whether long lines wrap at the editor's width; Markdown wraps always.
    pub wrap: bool,
}

impl Default for Editor {
    fn default() -> Editor {
        Editor {
            font_size: 13.0,
            vim: false,
            vim_clipboard: false,
            show_whitespace: false,
            wrap: false,
        }
    }
}

/// The file list of a workspace's sidebar entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Files {
    /// Whether files that git ignores and hidden ones show, dimmed.
    pub show_ignored: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_editor_section_reads_what_the_user_wrote_and_keeps_the_rest() {
        let (config, warnings) =
            crate::Config::parse("version = 1\n[editor]\nfont_size = 15\nvim = true\n[files]\nshow_ignored = true\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.editor.font_size, 15.0);
        assert!(config.editor.vim && !config.editor.wrap);
        assert!(config.files.show_ignored);
        let (plain, _) = crate::Config::parse("version = 1\n");
        assert_eq!(plain.editor, super::Editor::default());
    }
}
