//! The file list of a workspace's sidebar entry in the app (E22): the app is `term-ui-files`' `Host` for where a file
//! opens and what follows a change on disk.

use std::path::Path;

use term_ui::workspace_host::WorkspaceId;

/// The app as the file lists' host.
struct FilesHost;

static HOST: FilesHost = FilesHost;

impl term_ui_files::Host for FilesHost {
    fn open_file(&self, workspace: WorkspaceId, path: &Path, preview: bool) {
        crate::editor_tab::open_in(workspace, path, preview);
    }

    fn open_as_preview(&self, workspace: WorkspaceId, path: &Path) {
        crate::preview_tab::open_as_preview(workspace, path);
    }

    fn open_in_browser_tab(&self, workspace: WorkspaceId, path: &Path) {
        let url = objc2_foundation::NSURL::fileURLWithPath(&term_ui::kit::ns(&path.to_string_lossy()));
        let address = url.absoluteString().map(|address| address.to_string());
        let _ = term_ui_browser::open_tab(crate::state::app().mtm, workspace, address.as_deref());
    }

    fn changed(&self, root: &Path) {
        term_ui_git::changed(root);
    }

    fn show_ignored(&self) -> bool {
        crate::state::app().config().files.show_ignored
    }
}

/// Hands the file lists their host; once, before the first window opens.
pub(crate) fn install() {
    term_ui_files::install(&HOST);
}
