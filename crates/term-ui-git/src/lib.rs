//! The Git panel of a workspace, the diff tab and the branch item of the toolbar (E22).
//!
//! The panel is a view the app shows in the sidebar in place of the workspaces ([`panel_view`]), with the buttons of
//! its title row ([`panel_accessory`]); it lays out at the sidebar's width and opens diffs as tabs of the workspace. git runs off the main thread, neutralised while the user
//! does not trust the workspace (E16, E21); destructive operations, such as discarding or deleting a stash entry, ask
//! first.

mod commands;
mod commit_field;
mod diff;
mod diff_view;
mod graph;
mod history;
mod host;
mod hover;
mod kept;
mod model;
mod ops;
mod panel;
#[cfg(feature = "selftest")]
pub mod probe;
mod progress;
mod refresh;
mod reveal;
mod rows;
mod sheets;
mod stashes;
mod toolbar;
mod ui;
mod work;

use std::path::Path;

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_git::panel as text;
use term_ui::workspace_host::AsWorkspace;

pub use commands::menu;
pub use host::{Host, State, install};
#[cfg(feature = "selftest")]
pub use reveal::selected_commit;
pub use reveal::{reveal_commit, show_commit_diff};
pub use sheets::confirm_delete;
pub use text::ago;
pub use toolbar::{TOOLBAR_ID, toolbar_item};
#[cfg(feature = "selftest")]
pub use toolbar::{choose as toolbar_choose, seen as toolbar_seen};
pub use ui::empty_state;
pub use work::git;

/// The view of the workspace's Git panel, built for its base path the first time and the same view after; `None`
/// once the workspace closed.
pub fn panel_view(workspace: &(impl AsWorkspace + ?Sized)) -> Option<Retained<NSView>> {
    let workspace = &workspace.as_workspace();
    panel::GitPanel::attach(workspace).map(|panel| panel.ui().page.clone())
}

/// The buttons at the end of the title row of the workspace's Git panel, once [`panel_view`] built it.
pub fn panel_accessory(workspace: &(impl AsWorkspace + ?Sized)) -> Option<Retained<NSView>> {
    panel::of(workspace).map(|panel| Retained::into_super(panel.ui().header.clone()))
}

/// Lets go of the workspace's Git panel, as when the workspace closes or its panel stays shut; the next
/// [`panel_view`] builds it anew.
pub fn detach(workspace: &(impl AsWorkspace + ?Sized)) {
    host::state().panels.detach(workspace.as_workspace());
}

/// Something changed below `root` on disk, in the working tree or in `.git`.
pub fn changed(root: &Path) {
    for panel in panel::for_root(root) {
        panel.schedule_refresh();
    }
}

/// Whether the workspace's base path is a repository with a commit, as its Git panel read it last.
pub fn has_commit(workspace: &(impl AsWorkspace + ?Sized)) -> bool {
    panel::of(workspace).is_some_and(|panel| {
        let state = panel.read();
        state.phase == model::Phase::Ready && state.branch.commit.is_some()
    })
}

/// Reads git again for the workspace's Git panel, as after a change it made.
pub fn refresh(workspace: &(impl AsWorkspace + ?Sized)) {
    if let Some(panel) = panel::of(workspace) {
        panel.start_refresh();
    }
}

/// The user's trust in the workspace changed: its repository is found again under the new policy (E21).
pub fn trust_changed(workspace: &(impl AsWorkspace + ?Sized)) {
    if let Some(panel) = panel::of(workspace) {
        panel.trust_changed();
    }
}

/// What the self-test reads from and does with a workspace's Git panel, through the code its controls run.
#[cfg(feature = "selftest")]
pub mod selftest;
