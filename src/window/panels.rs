//! The panels of the workspaces (E22): a row's switch or a command shows the file list or the Git panel of a
//! workspace in the sidebar in place of the workspaces, its way back shows the list again, and the panel's view moves
//! into the sidebar of the tab window the user works in.

use std::rc::Rc;

use term_ui_sidebar::{Panel, PanelView};

use super::Window;
use super::frame::TabWindow;

impl Window {
    /// Shows `panel` of workspace `id` in place of the list, selecting the workspace, or the list again.
    pub(crate) fn toggle_panel(self: &Rc<Self>, id: u64, panel: Panel) {
        let open = self
            .workspaces
            .borrow()
            .get(id)
            .is_some_and(|workspace| workspace.panels.borrow().is_open(panel));
        self.set_panel_open(id, panel, !open);
    }

    /// Shows or hides `panel` of the selected workspace, as its command does.
    pub(crate) fn toggle_selected_panel(self: &Rc<Self>, panel: Panel) {
        if self.is_quick() {
            return objc2_app_kit::NSBeep();
        }
        if let Some(workspace) = self.selected_workspace() {
            self.toggle_panel(workspace.id, panel);
        }
    }

    /// Shows `panel` of workspace `id` in place of the list, which selects the workspace, or the list again and lets
    /// go of what the panel holds.
    pub(crate) fn set_panel_open(self: &Rc<Self>, id: u64, panel: Panel, open: bool) {
        let Some(workspace) = self.workspaces.borrow().get(id) else {
            return;
        };
        if workspace.panels.borrow().is_open(panel) == open {
            return;
        }
        let before = workspace.panels.borrow().shown();
        workspace.panels.borrow_mut().set_open(panel, open);
        if open && !self.sidebar_shown() {
            self.toggle_sidebar();
        }
        if open && self.selected_workspace().is_none_or(|selected| selected.id != id) {
            self.show_workspace(id, self.front_frame());
        }
        self.refresh();
        for gone in before.into_iter().filter(|gone| !(open && *gone == panel)) {
            crate::panels::closed(&workspace.handle(), gone);
        }
    }

    /// Shows `count` changed files on the Git symbol of workspace `id`'s row.
    pub(crate) fn set_change_count(&self, id: u64, count: usize) {
        let changed = self
            .workspaces
            .borrow()
            .get(id)
            .is_some_and(|workspace| workspace.set_changes(count));
        if changed {
            self.refresh();
        }
    }

    /// Shows the selected workspace's panel in the sidebar of the tab window the user works in, and the list in the
    /// sidebars of the workspaces without one.
    pub(super) fn show_panels(&self) {
        let target = self.panel_frame();
        let frames = self.frames.borrow().clone();
        for frame in &frames {
            let shown = self
                .workspaces
                .borrow()
                .get(frame.workspace)
                .and_then(|workspace| workspace.panels.borrow().shown());
            let is_target = target.as_ref().is_some_and(|target| target.tab == frame.tab);
            let view = shown.and_then(|_| self.panel_view(frame.workspace, is_target));
            frame.sidebar.set_panel(view);
        }
    }

    /// Workspace `id`'s panel that shows, with its title; its view and buttons only `with_content`.
    fn panel_view(&self, id: u64, with_content: bool) -> Option<PanelView> {
        let workspace = self.workspaces.borrow().get(id)?;
        let panel = workspace.panels.borrow().shown()?;
        let handle = workspace.handle();
        let content = if with_content {
            Some(crate::panels::view(&handle, panel)?)
        } else {
            None
        };
        Some(PanelView {
            workspace: id,
            panel,
            title: crate::panels::title(panel, &workspace.name()),
            content,
            accessory: with_content.then(|| crate::panels::accessory(&handle, panel)).flatten(),
        })
    }

    /// The tab window whose sidebar shows the panel: the one that had the keyboard last while it is on screen, else
    /// the one in front.
    fn panel_frame(&self) -> Option<Rc<TabWindow>> {
        self.last_key
            .get()
            .and_then(|tab| self.frame_of(tab))
            .filter(|frame| frame.ns.isVisible())
            .or_else(|| self.front_frame().filter(|frame| frame.ns.isVisible()))
    }

    /// Goes back from the panel to the workspaces in the sidebar of the window in front, as its button does.
    #[cfg(feature = "selftest")]
    pub(crate) fn panel_back(&self) {
        if let Some(frame) = self.front_frame() {
            frame.sidebar.go_back();
        }
    }

    /// Clicks the settings button of the sidebar of the window in front; its tooltip while it shows.
    #[cfg(feature = "selftest")]
    pub(crate) fn press_settings(&self) -> Option<String> {
        self.front_frame()?.sidebar.press_settings()
    }

    /// What the sidebar of the window in front shows: the list, or a panel's title, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn sidebar_state(&self) -> String {
        let Some(frame) = self.front_frame() else {
            return "no window".into();
        };
        frame
            .sidebar
            .shown_panel()
            .map_or_else(|| "list".into(), |(_, panel, title)| format!("{panel:?} \"{title}\""))
    }
}
