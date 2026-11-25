//! The app as the UI crates' workspace host (E3, E20): a workspace by its id is one of a window's workspaces, its
//! base path the project, its tabs those of its tab bar.

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView, NSWindow};
use term_ui::workspace_host::{
    AgentStatus, AppEnvHost, FilesHost, IdentityHost, NoticesHost, TabDecorHost, TabId, TabKind, TabsHost, TrustHost,
    WorkspaceId,
};

use super::find;
use crate::state::try_app;

/// The app's answer to what a workspace is.
pub(crate) struct Workspaces;

impl IdentityHost for Workspaces {
    fn is_open(&self, workspace: WorkspaceId) -> bool {
        find(workspace).is_some()
    }

    fn frontmost(&self) -> Option<WorkspaceId> {
        let workspace = try_app()?.front()?.selected_workspace()?;
        Some(WorkspaceId::new(workspace.id))
    }

    fn ns_window(&self, workspace: WorkspaceId) -> Option<Retained<NSWindow>> {
        find(workspace)?.0.front_ns_window()
    }

    fn project_root(&self, workspace: WorkspaceId) -> Option<PathBuf> {
        Some(find(workspace)?.1.base.clone())
    }
}

impl TrustHost for Workspaces {
    fn is_trusted(&self, workspace: WorkspaceId) -> bool {
        find(workspace).is_some_and(|(_, workspace)| workspace.is_trusted())
    }

    fn ask_trust_again(&self, workspace: WorkspaceId) {
        super::ask_trust(workspace);
    }
}

impl TabsHost for Workspaces {
    fn add_tab(
        &self,
        workspace: WorkspaceId,
        kind: TabKind,
        key: Option<String>,
        title: &str,
        view: Retained<NSView>,
    ) -> Option<TabId> {
        let (window, _) = find(workspace)?;
        window
            .add_hosted(workspace.get(), kind, key, title, view)
            .map(TabId::new)
    }

    fn select_tab_with_key(&self, workspace: WorkspaceId, key: &str) -> bool {
        let Some((window, _)) = find(workspace) else {
            return false;
        };
        let Some(tab) = window.hosted_with_key(workspace.get(), key) else {
            return false;
        };
        window.select(tab.id);
        true
    }

    fn set_tab_title(&self, workspace: WorkspaceId, tab: TabId, title: &str) {
        if let Some((window, _)) = find(workspace) {
            window.set_hosted_title(workspace.get(), tab.get(), title);
        }
    }

    fn select_tab(&self, workspace: WorkspaceId, tab: TabId) {
        if let Some((window, _)) = find(workspace)
            && window.hosted_tab(workspace.get(), tab.get()).is_some()
        {
            window.select(tab.get());
        }
    }

    fn close_tab(&self, workspace: WorkspaceId, tab: TabId) {
        if let Some((window, _)) = find(workspace) {
            window.close_hosted(workspace.get(), tab.get());
        }
    }

    fn selected_tab(&self, workspace: WorkspaceId) -> Option<TabId> {
        let (_, workspace) = find(workspace)?;
        workspace.selected_tab().map(|tab| TabId::new(tab.id))
    }

    fn tab_with_key(&self, workspace: WorkspaceId, key: &str) -> Option<TabId> {
        let (window, _) = find(workspace)?;
        let tab = window.hosted_with_key(workspace.get(), key)?;
        Some(TabId::new(tab.id))
    }

    fn rekey_tab(&self, workspace: WorkspaceId, tab: TabId, key: Option<String>) {
        if let Some((window, _)) = find(workspace) {
            window.rekey_hosted(workspace.get(), tab.get(), key);
        }
    }
}

impl TabDecorHost for Workspaces {
    fn set_tab_status(&self, workspace: WorkspaceId, tab: TabId, status: AgentStatus) {
        if let Some((window, _)) = find(workspace) {
            window.set_hosted_status(workspace.get(), tab.get(), status);
        }
    }

    fn set_tab_tint(&self, workspace: WorkspaceId, tab: TabId, color: &NSColor) {
        if let Some((window, _)) = find(workspace) {
            window.set_hosted_colours(workspace.get(), tab.get(), Some(color), None);
        }
    }

    fn set_tab_four_colours(&self, workspace: WorkspaceId, tab: TabId, colors: [Retained<NSColor>; 4]) {
        if let Some((window, _)) = find(workspace) {
            window.set_hosted_colours(workspace.get(), tab.get(), None, Some(colors));
        }
    }
}

impl NoticesHost for Workspaces {
    fn show_notice(&self, workspace: WorkspaceId, text: &str) {
        if let Some((window, _)) = find(workspace) {
            window.show_notice(text);
        }
    }

    fn refresh_activity(&self, workspace: WorkspaceId) {
        if let Some((window, _)) = find(workspace) {
            window.refresh_hosted();
        }
    }
}

impl FilesHost for Workspaces {
    fn open_file(&self, workspace: WorkspaceId, path: &Path) {
        if find(workspace).is_some() {
            crate::editor_tab::open_in(workspace, path, false);
        } else {
            crate::open::file(path);
        }
    }
}

impl AppEnvHost for Workspaces {
    fn environment(&self, wait: std::time::Duration) -> Option<Vec<(String, String)>> {
        super::env::environment(wait)
    }
}
