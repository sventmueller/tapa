//! The tabs of a window whose views UI crates hand over through the workspace host (E21): adding them to a
//! workspace as a window of its tab group, naming them and showing their status. Each view stays in its tab's window
//! until the tab closes, also while its workspace is off screen, since an agent's view that leaves its window for good
//! ends its session.

use std::rc::Rc;
use term_ui::workspace_host::{AgentStatus, TabId, TabKind, WorkspaceId};

use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::Window;
use crate::hosted::Hosted;
use crate::state::app;
use crate::tab::Tab;

impl Window {
    /// Adds `view` as a tab of workspace `workspace` after its tab in front and brings it forward; its id, or none
    /// when the window does not hold the workspace.
    pub(crate) fn add_hosted(
        self: &Rc<Self>,
        workspace: u64,
        kind: TabKind,
        key: Option<String>,
        title: &str,
        view: Retained<NSView>,
    ) -> Option<u64> {
        let workspace = self.workspaces.borrow().get(WorkspaceId::new(workspace))?;
        let hosted = Hosted::new(kind, key, title, view, workspace.base.clone());
        let tab = Rc::new(Tab::hosted(app().next_id(), hosted));
        let anchor = workspace.tabs.borrow().selected_id().map(TabId::get);
        if let Some(anchor) = anchor.and_then(|id| self.frame_of(id)) {
            self.sync_group_order(&anchor);
        }
        workspace.tabs.borrow_mut().insert_after_selected(tab.clone());
        self.attach(&tab, workspace.id, anchor, None, None);
        if !self.restoring.get() {
            self.select(tab.id);
        }
        Some(tab.id)
    }

    /// The hosted tab `tab` of workspace `workspace`.
    pub(crate) fn hosted_tab(&self, workspace: u64, tab: u64) -> Option<Rc<Tab>> {
        let tab = self
            .workspaces
            .borrow()
            .get(WorkspaceId::new(workspace))?
            .tabs
            .borrow()
            .get(TabId::new(tab))?;
        tab.as_hosted().is_some().then_some(tab)
    }

    /// The hosted tab of workspace `workspace` known by `key`.
    pub(crate) fn hosted_with_key(&self, workspace: u64, key: &str) -> Option<Rc<Tab>> {
        let workspace = self.workspaces.borrow().get(WorkspaceId::new(workspace))?;
        workspace.tabs().into_iter().find(|tab| {
            tab.as_hosted()
                .is_some_and(|hosted| hosted.key().as_deref() == Some(key))
        })
    }

    /// Names the hosted tab `tab` of workspace `workspace` `title`.
    pub(crate) fn set_hosted_title(&self, workspace: u64, tab: u64, title: &str) {
        let tab = self.hosted_tab(workspace, tab);
        if tab.and_then(|tab| tab.as_hosted().map(|hosted| hosted.set_title(title))) == Some(true) {
            self.refresh();
        }
    }

    /// Shows `status` on the hosted tab `tab` of workspace `workspace` and in the workspace's row.
    /// Gives the symbol of hosted tab `tab` in the tab bar `tint`, or the colours of its four points.
    pub(crate) fn set_hosted_colours(
        &self,
        workspace: u64,
        tab: u64,
        tint: Option<&objc2_app_kit::NSColor>,
        four: Option<[objc2::rc::Retained<objc2_app_kit::NSColor>; 4]>,
    ) {
        let Some(tab) = self.hosted_tab(workspace, tab) else {
            return;
        };
        let Some(hosted) = tab.as_hosted() else { return };
        if let Some(tint) = tint {
            hosted.set_tint(tint);
        }
        if let Some(four) = four {
            hosted.set_four_colours(four);
        }
        if let Some(frame) = self.frame_of(tab.id) {
            frame.invalidate_tab_title();
        }
        self.refresh();
    }

    pub(crate) fn set_hosted_status(&self, workspace: u64, tab: u64, status: AgentStatus) {
        let Some(tab) = self.hosted_tab(workspace, tab) else {
            return;
        };
        let in_front =
            self.selected_tab_id() == Some(tab.id) && self.front_ns_window().is_some_and(|window| window.isKeyWindow());
        let status = match status {
            AgentStatus::Attention if in_front => AgentStatus::Idle,
            other => other,
        };
        if tab.as_hosted().is_some_and(|hosted| hosted.set_status(status)) {
            self.refresh();
        }
    }

    /// Marks whether the file of the hosted tab `tab` of workspace `workspace` has unsaved changes, as its window's
    /// close button and tab show it.
    pub(crate) fn set_hosted_dirty(&self, workspace: u64, tab: u64, dirty: bool) {
        let Some(tab) = self.hosted_tab(workspace, tab) else {
            return;
        };
        if tab.as_hosted().is_some_and(|hosted| hosted.set_dirty(dirty))
            && let Some(frame) = self.frame_of(tab.id)
        {
            frame.ns.setDocumentEdited(dirty);
        }
    }

    /// Knows the hosted tab `tab` of workspace `workspace` by `key` from now on.
    pub(crate) fn rekey_hosted(&self, workspace: u64, tab: u64, key: Option<String>) {
        if let Some(hosted) = self.hosted_tab(workspace, tab).as_deref().and_then(Tab::as_hosted) {
            hosted.set_key(key);
        }
    }

    /// Closes the hosted tab `tab` of workspace `workspace` without asking, as what runs in it asked for that.
    pub(crate) fn close_hosted(self: &Rc<Self>, workspace: u64, tab: u64) {
        if let Some(tab) = self.hosted_tab(workspace, tab) {
            self.remove_tab(&tab);
        }
    }

    /// Shows `text` as the window's subtitle for a few seconds; VoiceOver reads it.
    pub(crate) fn show_notice(&self, text: &str) {
        let Some(ns_window) = self.front_ns_window() else {
            return;
        };
        ns_window.setSubtitle(&term_ui::kit::ns(text));
        super::title::notice(&ns_window, text);
        term_ui_agent::common::panel::announce(self.mtm, text);
        let id = self.id();
        term_base::main_after(NOTICE_SHOWS, move || {
            if let Some(window) = crate::state::try_app().and_then(|app| app.window(id)) {
                window.refresh();
            }
        });
    }

    /// Shows the sidebar and the tab bar again, as what a hosted tab runs changed.
    pub(crate) fn refresh_hosted(&self) {
        self.refresh();
    }
}

/// How long a notice stays in the window's subtitle.
const NOTICE_SHOWS: std::time::Duration = std::time::Duration::from_secs(4);
