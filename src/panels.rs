//! The file list and the Git panel of a workspace (E22): which of them shows in the sidebar in place of the
//! workspaces while the workspace is selected, kept per workspace and restored with it; and the app as the Git panels'
//! host.
//!
//! Each workspace has one view per panel, built by its crate; it shows in the sidebar of the window the user works in
//! and moves to the next one's as another tab window takes the keyboard, since a view lives in one window only.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use splat_i18n::tf;
use term_ui::workspace_host::{Workspace as Handle, WorkspaceId};
use term_ui_sidebar::Panel;

use crate::state::try_app;

/// One panel of a workspace: whether it shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub(crate) open: bool,
}

/// The two panels of a workspace, of which at most one shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Panels {
    #[serde(default)]
    pub(crate) files: State,
    #[serde(default)]
    pub(crate) git: State,
}

impl Panels {
    fn state(&mut self, panel: Panel) -> &mut State {
        match panel {
            Panel::Files => &mut self.files,
            Panel::Git => &mut self.git,
        }
    }

    fn get(&self, panel: Panel) -> State {
        match panel {
            Panel::Files => self.files,
            Panel::Git => self.git,
        }
    }

    /// The panel that shows; the file list where an older restore left both open.
    pub(crate) fn shown(&self) -> Option<Panel> {
        Panel::ALL.into_iter().find(|panel| self.get(*panel).open)
    }

    /// The panel that shows, as a list.
    pub(crate) fn open(&self) -> Vec<Panel> {
        self.shown().into_iter().collect()
    }

    pub(crate) fn is_open(&self, panel: Panel) -> bool {
        self.shown() == Some(panel)
    }

    /// Shows `panel` in place of the other, or neither.
    pub(crate) fn set_open(&mut self, panel: Panel, open: bool) {
        if open {
            for other in Panel::ALL {
                self.state(other).open = false;
            }
        }
        self.state(panel).open = open;
    }
}

/// The title of `panel` of the workspace named `name`, as Dango names its panels.
pub(crate) fn title(panel: Panel, name: &str) -> String {
    match panel {
        Panel::Files => tf("Dateien: {}", &[name]),
        Panel::Git => "Git".to_string(),
    }
}

/// The buttons at the end of the title row of `panel` of `workspace`, once its view is built.
pub(crate) fn accessory(workspace: &Handle, panel: Panel) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    match panel {
        Panel::Files => term_ui_files::panel_accessory(workspace),
        Panel::Git => term_ui_git::panel_accessory(workspace),
    }
}

/// The view of `panel` of `workspace`, built the first time; the file list's start its watcher.
pub(crate) fn view(workspace: &Handle, panel: Panel) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    match panel {
        Panel::Files => term_ui_files::panel_view(workspace),
        Panel::Git => term_ui_git::panel_view(workspace),
    }
}

/// Lets go of the view of `panel` of `workspace` as it closes: the file list with its watcher. The Git panel stays
/// while the workspace is open, as the toolbar's branch and the count on its symbol read it; it watches nothing.
pub(crate) fn closed(workspace: &Handle, panel: Panel) {
    if panel == Panel::Files {
        term_ui_files::detach(workspace);
    }
}

/// Lets go of both panels of `workspace`, which closed.
pub(crate) fn workspace_closed(workspace: &Handle) {
    term_ui_files::detach(workspace);
    term_ui_git::detach(workspace);
}

/// The app as the Git panels' host.
struct GitHost;

static GIT: GitHost = GitHost;

thread_local! {
    static GIT_STATE: Rc<term_ui_git::State> = Rc::new(term_ui_git::State::default());
}

impl term_ui_git::Host for GitHost {
    fn state(&self) -> Rc<term_ui_git::State> {
        GIT_STATE.with(Rc::clone)
    }

    fn unsaved_below(&self, root: &Path) -> Vec<PathBuf> {
        term_ui_editor::unsaved_below(root)
    }

    fn save_below(&self, root: &Path, then: Box<dyn FnOnce(bool)>) {
        term_ui_editor::save_below(root, then);
    }

    fn check_disk_below(&self, root: &Path) {
        term_ui_editor::check_disk_below(root);
    }

    fn show_panel(&self, workspace: WorkspaceId) {
        if let Some(window) = window_of(workspace) {
            window.set_panel_open(workspace.get(), Panel::Git, true);
        }
    }

    fn set_change_count(&self, workspace: WorkspaceId, count: usize) {
        if let Some(window) = window_of(workspace) {
            window.set_change_count(workspace.get(), count);
        }
    }

    fn open_workspace(&self, path: &Path) {
        let Some(app) = try_app() else { return };
        match app.front() {
            Some(window) => window.open_workspace(path.to_path_buf()),
            None => crate::window::Window::open(None, None).open_workspace(path.to_path_buf()),
        }
    }
}

/// Hands the Git panels their host; once, before the first window opens.
pub(crate) fn install() {
    term_ui_git::install(&GIT);
}

/// The window that holds workspace `workspace`.
fn window_of(workspace: WorkspaceId) -> Option<Rc<crate::window::Window>> {
    let app = try_app()?;
    let windows = app.windows.borrow();
    windows
        .iter()
        .find(|window| window.workspaces().iter().any(|own| own.id == workspace.get()))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_panel_shows_at_a_time() {
        let mut panels = Panels::default();
        assert_eq!(panels.shown(), None);
        panels.set_open(Panel::Git, true);
        assert_eq!(panels.open(), [Panel::Git]);
        panels.set_open(Panel::Files, true);
        assert_eq!(
            panels.shown(),
            Some(Panel::Files),
            "the file list takes the Git panel's place"
        );
        assert!(!panels.is_open(Panel::Git));
        panels.set_open(Panel::Files, false);
        assert_eq!(panels.shown(), None);
    }
}
