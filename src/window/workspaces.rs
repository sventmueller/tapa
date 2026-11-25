//! The workspaces of a window (E20): a new one for a folder the user picks, choosing and stepping through them, which
//! swaps the tab group on screen, renaming and the menus of the sidebar's rows.

use std::path::PathBuf;
use std::rc::Rc;
use term_ui::workspace_host::WorkspaceId;

use block2::RcBlock;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel};
use objc2_foundation::NSURL;
use splat_i18n::t;
use term_base::guarded;
use term_ui::kit::alert::{Alert, Role};
use term_ui::kit::ns;
use term_ui_sidebar::RowAction;

use super::{Window, session};
use crate::state::{app, try_app};
use crate::workspace::Workspace;

impl Window {
    /// Adds a workspace without tabs in `base` after the selected one, which stays selected.
    pub(super) fn add_workspace(&self, base: PathBuf, name: Option<String>) -> Rc<Workspace> {
        let workspace = Rc::new(Workspace::new(app().next_id(), base, name));
        self.workspaces.borrow_mut().insert_after_selected(workspace.clone());
        workspace
    }

    /// Asks where a new workspace works, a folder on this Mac or another machine (E25), and opens it there.
    pub(crate) fn new_workspace(&self) {
        if self.is_quick() {
            objc2_app_kit::NSBeep();
            return;
        }
        let Some(ns_window) = self.front_ns_window() else {
            return;
        };
        let id = self.id();
        Alert::new(self.mtm, t("Wo arbeitet der neue Arbeitsbereich?"))
            .button(t("Ordner auf diesem Mac …"), Role::Plain)
            .button(t("Entfernter Rechner …"), Role::Plain)
            .button(t("Abbrechen"), Role::CancelOnEscape)
            .sheet(&ns_window, move |response| {
                // The next sheet waits until this one has gone.
                term_base::main_soon(move || {
                    let Some(window) = try_app().and_then(|app| app.window(id)) else {
                        return;
                    };
                    if response == objc2_app_kit::NSAlertFirstButtonReturn {
                        window.new_local_workspace();
                    } else if response == objc2_app_kit::NSAlertSecondButtonReturn {
                        window.new_remote_workspace();
                    }
                });
            });
    }

    /// Asks for the machine of a new remote workspace in a sheet and opens it (E25).
    pub(crate) fn new_remote_workspace(&self) {
        let Some(ns_window) = self.front_ns_window() else {
            return;
        };
        let id = self.id();
        crate::remote::sheet::ask(&ns_window, None, move |target, password| {
            if let Some(window) = try_app().and_then(|app| app.window(id)) {
                if !password.is_empty() {
                    crate::remote::keep_password(&target, &password);
                }
                window.open_remote_workspace(target);
            }
        });
    }

    /// Brings the remote workspace on `target` forward, or opens one with a tab and connects it (E25).
    pub(crate) fn open_remote_workspace(self: &Rc<Self>, target: term_remote::Target) {
        let known = self.workspaces().into_iter().find(|workspace| {
            workspace
                .remote_machine()
                .is_some_and(|remote| remote.target() == target)
        });
        if let Some(workspace) = known {
            return self.select_workspace(workspace.id);
        }
        let remote = crate::remote::Remote::new(target);
        remote.connect();
        let workspace = Rc::new(Workspace::remote(app().next_id(), remote, None));
        self.workspaces.borrow_mut().insert_after_selected(workspace.clone());
        self.select_workspace(workspace.id);
        crate::restore::save();
    }

    /// Asks for the folder of a new workspace in a sheet, starting at the selected workspace's, and opens it.
    pub(crate) fn new_local_workspace(&self) {
        let Some(ns_window) = self.front_ns_window() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(self.mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setCanCreateDirectories(true);
        panel.setMessage(Some(&ns(t(
            "Der Ordner des Arbeitsbereichs, in dem seine Tabs starten.",
        ))));
        let start = self
            .selected_workspace()
            .map_or_else(default_base, |workspace| workspace.base.clone());
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&ns(&start.to_string_lossy()))));
        let chosen = panel.clone();
        let id = self.id();
        let handler = RcBlock::new(move |response: NSModalResponse| {
            guarded((), || {
                if response != NSModalResponseOK {
                    return;
                }
                let Some(path) = chosen.URL().and_then(|url| url.path()) else {
                    return;
                };
                if let Some(window) = try_app().and_then(|app| app.window(id)) {
                    window.open_workspace(PathBuf::from(path.to_string()));
                }
            });
        });
        panel.beginSheetModalForWindow_completionHandler(&ns_window, &handler);
    }

    /// Brings the workspace in `base` forward, or opens one there with a tab after the selected one; either takes the
    /// place of the tab group in front.
    pub(crate) fn open_workspace(self: &Rc<Self>, base: PathBuf) {
        crate::recent::used(&base);
        let known = self
            .workspaces()
            .into_iter()
            .find(|workspace| !workspace.scratch && workspace.base == base);
        if let Some(workspace) = known {
            return self.select_workspace(workspace.id);
        }
        let workspace = self.add_workspace(base, None);
        self.select_workspace(workspace.id);
        crate::restore::save();
    }

    /// Brings workspace `id` forward with its tab in front, in place of the tab group in front.
    pub(crate) fn select_workspace(self: &Rc<Self>, id: u64) {
        let replacing = self.last_key.get().and_then(|tab| self.frame_of(tab));
        self.show_workspace(id, replacing.or_else(|| self.front_frame()));
        self.focus_active();
    }

    /// Selects the workspace `step` places from the selected one, wrapping around.
    pub(crate) fn step_workspace(self: &Rc<Self>, step: isize) {
        let next = self.workspaces.borrow().step(step).map(WorkspaceId::get);
        if let Some(id) = next {
            self.select_workspace(id);
        }
    }

    /// Turns the name of workspace `id`, or of the selected one, into a field in its row of the sidebar in front.
    pub(crate) fn rename_workspace(&self, id: Option<u64>) {
        let Some(id) = id.or_else(|| self.selected_workspace().map(|workspace| workspace.id)) else {
            return;
        };
        if let Some(frame) = self.front_frame() {
            frame.sidebar.begin_rename(id);
        }
    }

    /// Names workspace `id` `name`; an empty name gives it its folder's again.
    pub(crate) fn set_workspace_name(&self, id: u64, name: &str) {
        if let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(id)) {
            workspace.rename(name);
        }
        self.refresh();
        crate::restore::save();
    }

    /// Moves workspace `id` to place `index` in the sidebar, as the user dragged its row; never above the Terminal.
    pub(crate) fn move_workspace(&self, id: u64, index: usize) {
        {
            let mut workspaces = self.workspaces.borrow_mut();
            let fixed = workspaces
                .items()
                .iter()
                .take_while(|workspace| workspace.scratch)
                .count();
            if workspaces
                .get(WorkspaceId::new(id))
                .is_none_or(|workspace| workspace.scratch)
            {
                return;
            }
            workspaces.move_to(WorkspaceId::new(id), index.max(fixed));
        }
        self.refresh();
        crate::restore::save();
    }

    /// Drags the row at `from` in the sidebar in front to place `to`, as the mouse does, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn drag_row(&self, from: usize, to: usize) {
        let Some(id) = self.workspaces().get(from).map(|workspace| workspace.id) else {
            return;
        };
        if let Some(frame) = self.front_frame() {
            frame.sidebar.drop_row(id, if to > from { to + 1 } else { to });
        }
    }

    /// The sidebar in front and its window, for the self-test's mouse.
    #[cfg(feature = "selftest")]
    pub(crate) fn front_sidebar(
        &self,
    ) -> Option<(
        objc2::rc::Retained<term_ui_sidebar::Sidebar>,
        objc2::rc::Retained<objc2_app_kit::NSWindow>,
    )> {
        let frame = self.front_frame()?;
        Some((frame.sidebar.clone(), frame.ns.clone()))
    }

    /// Does what the user chose in the context menu of workspace `id`'s row.
    pub(crate) fn row_action(self: &Rc<Self>, id: u64, action: RowAction) {
        match action {
            RowAction::NewTab => {
                self.select_workspace(id);
                self.new_tab();
            }
            RowAction::Rename => self.rename_workspace(Some(id)),
            RowAction::Close => self.close_workspace(id),
            RowAction::ToggleTrust => {
                if let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(id)) {
                    workspace.set_trusted(!workspace.is_trusted());
                }
                self.refresh();
            }
            RowAction::Reconnect => {
                if let Some(remote) = self.remote_of(id) {
                    remote.connect();
                }
            }
            RowAction::Disconnect => {
                if let Some(remote) = self.remote_of(id) {
                    remote.disconnect();
                }
            }
            RowAction::EditConnection => self.edit_connection(id),
        }
    }

    fn remote_of(&self, id: u64) -> Option<Rc<crate::remote::Remote>> {
        self.workspaces
            .borrow()
            .get(WorkspaceId::new(id))
            .and_then(|workspace| workspace.remote_machine())
    }

    /// Changes where remote workspace `id` connects to, in a sheet filled with its machine now.
    fn edit_connection(&self, id: u64) {
        let (Some(remote), Some(ns_window)) = (self.remote_of(id), self.front_ns_window()) else {
            return;
        };
        let weak = Rc::downgrade(&remote);
        crate::remote::sheet::ask(&ns_window, Some(&remote.target()), move |target, password| {
            if !password.is_empty() {
                crate::remote::keep_password(&target, &password);
            }
            if let Some(remote) = weak.upgrade() {
                remote.retarget(target);
            }
            crate::restore::save();
        });
    }

    /// Trusts the selected workspace, or takes the trust back (E21).
    pub(crate) fn toggle_trust(self: &Rc<Self>) {
        if let Some(workspace) = self.selected_workspace() {
            self.row_action(workspace.id, RowAction::ToggleTrust);
        }
    }
}

/// The folder a new workspace's panel starts in without a workspace: the default profile's, else the home folder.
fn default_base() -> PathBuf {
    let config = app().config();
    session::start_folder(&config, None, false)
}
