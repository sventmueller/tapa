//! The app as the editors reach it: their settings, the key of the recovery copies, the unsaved mark of a tab and what
//! else follows an editor; the rest of a workspace they reach through `term_ui::workspace_host`.

use std::path::Path;
use std::sync::OnceLock;

use objc2::DefinedClass;
use term_ui::workspace_host::{TabId, WorkspaceId};

use crate::text_view::EditorSerial;

/// The settings the editors read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The size of the code font in points.
    pub font_size: f64,
    /// Whether editors take Vim's keys (E105).
    pub vim_mode: bool,
    /// Whether Vim's unnamed register is the system pasteboard.
    pub vim_clipboard: bool,
    /// Whether spaces, tabs and line ends show.
    pub show_whitespace: bool,
    /// Whether long lines wrap at the editor's width in a new editor; Markdown wraps always.
    pub wrap: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            font_size: 13.0,
            vim_mode: false,
            vim_clipboard: false,
            show_whitespace: false,
            wrap: false,
        }
    }
}

/// What the editors need of the app beyond the workspace they are a tab of.
pub trait Host {
    /// The editors' settings as the user chose them.
    fn settings(&self) -> Settings;
    /// Keeps the user's choice of Vim's keys.
    fn set_vim_mode(&self, on: bool);
    /// Keeps the user's choice of showing whitespace.
    fn set_show_whitespace(&self, on: bool);
    /// The key that seals the copies of unsaved text against a crash (E17), looked up once; `None` keeps no copies.
    fn recovery_key(&self) -> Option<[u8; term_seal::KEY_LEN]>;
    /// The folder the copies of unsaved text go to.
    fn recovery_dir(&self) -> std::path::PathBuf {
        term_base::data_dir().join("recovery")
    }
    /// Shows on `tab` whether its text has unsaved changes.
    fn set_tab_dirty(&self, workspace: WorkspaceId, tab: TabId, dirty: bool);
    /// The editor `editor` wrote `path`, as the file list and the Git panel follow.
    fn saved(&self, workspace: WorkspaceId, path: &Path);
    /// The user opened `path` in `editor`, as a Markdown file's preview beside it follows when the user wants that.
    fn opened(&self, _workspace: WorkspaceId, _editor: EditorSerial, _path: &Path) {}
    /// The text of `editor`, whose file is `path`, changed, which a preview beside it follows.
    fn edited(&self, _editor: EditorSerial, _path: Option<&Path>) {}
    /// "Vorschau daneben ein-/ausblenden" in the Markdown document of `editor`.
    fn toggle_preview(&self, _workspace: WorkspaceId, _editor: EditorSerial) {}
    /// Whether a preview beside `editor` shows.
    fn preview_shown(&self, _editor: EditorSerial) -> bool {
        false
    }
    /// The editor `editor` is about to close, as its preview goes with it.
    fn closing(&self, _editor: EditorSerial) {}
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the editors belong to; once, before the first editor opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

pub(crate) fn host() -> &'static (dyn Host + Sync) {
    *HOST
        .get()
        .expect("the app installs its editor host before the first editor")
}

/// The editors' settings, or the defaults while no app installed a host, as in the crate's tests.
pub(crate) fn settings() -> Settings {
    HOST.get().map(|host| host.settings()).unwrap_or_default()
}

/// The text of `view` changed.
pub(crate) fn edited(view: &crate::EditorTextView) {
    if let Some(host) = HOST.get() {
        host.edited(view.ivars().serial, view.document().path());
    }
}

/// The host, if the app installed one.
pub(crate) fn installed() -> Option<&'static (dyn Host + Sync)> {
    HOST.get().copied()
}
