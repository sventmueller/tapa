//! The app as the file list reaches it: where a file opens and what follows a change on disk; the rest of a workspace
//! it reaches through `term_ui::workspace_host`.

use std::path::Path;
use std::sync::OnceLock;

use term_ui::workspace_host::{Workspace, WorkspaceId};

/// What the file list needs of the app beyond the workspace it belongs to.
pub trait Host {
    /// Opens `path` in a tab of `workspace`, in the workspace's preview tab with `preview`; an editor tab unless the
    /// app shows the kind of file otherwise.
    fn open_file(&self, workspace: WorkspaceId, path: &Path, preview: bool) {
        term_ui_editor::open_tab(&Workspace::new(workspace), path, preview);
    }
    /// "Als Vorschau öffnen": shows the drawing of the SVG file `path`.
    fn open_as_preview(&self, workspace: WorkspaceId, path: &Path);
    /// "Im Browser-Tab öffnen": shows the page `path` in a browser tab of `workspace`.
    fn open_in_browser_tab(&self, workspace: WorkspaceId, path: &Path);
    /// Something changed below `root` on disk, in the working tree or in `.git`, as the Git panel follows.
    fn changed(&self, root: &Path);
    /// Whether the list shows files that git ignores and hidden ones, dimmed.
    fn show_ignored(&self) -> bool {
        false
    }
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the file lists belong to; once, before the first list opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

/// The host, if the app installed one.
pub(crate) fn installed() -> Option<&'static (dyn Host + Sync)> {
    HOST.get().copied()
}
