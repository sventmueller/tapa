//! The app as the Git panel reaches it: its editors and the workspace's entry in the sidebar; the rest of a workspace
//! it reaches through `term_ui::workspace_host`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use objc2::rc::{Retained, Weak};
use term_ui::kit::registry::Registry;
use term_ui::workspace_host::WorkspaceId;

/// The Git panels, diff tabs and branch toolbar items of every workspace.
#[derive(Default)]
pub struct State {
    pub(crate) panels: Registry<Retained<crate::panel::GitPanel>>,
    pub(crate) diff_tabs: Registry<Weak<crate::diff::DiffTab>>,
    pub(crate) toolbar_items: RefCell<Vec<crate::toolbar::Shown>>,
}

/// What the Git panel needs of the app's editors and of the workspace's entry in the sidebar.
pub trait Host {
    /// The Git panel's state in the running app.
    fn state(&self) -> Rc<State>;
    /// The files below `root` with edits not yet saved.
    fn unsaved_below(&self, root: &Path) -> Vec<PathBuf>;
    /// Saves the open documents below `root` and calls `then` on the main thread once the writes are over, with
    /// whether all of them were saved.
    fn save_below(&self, root: &Path, then: Box<dyn FnOnce(bool)>);
    /// The open editors below `root` take what changed on disk.
    fn check_disk_below(&self, root: &Path);
    /// Opens the Git panel in the workspace's sidebar entry, with [`crate::panel_view`] for its content.
    fn show_panel(&self, workspace: WorkspaceId);
    /// How many files of the workspace's repository have changes, for the Git symbol of its sidebar row.
    fn set_change_count(&self, workspace: WorkspaceId, count: usize);
    /// Opens `path`, a new worktree of the workspace's repository, as a workspace of its own.
    fn open_workspace(&self, path: &Path);
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the Git panels belong to; once, before the first workspace opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

pub(crate) fn host() -> &'static (dyn Host + Sync) {
    *HOST
        .get()
        .expect("the app installs its host before the first Git panel")
}

pub(crate) fn state() -> Rc<State> {
    host().state()
}
