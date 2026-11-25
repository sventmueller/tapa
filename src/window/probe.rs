//! What the self-test reads of a window's native tab groups and sets on them.

use std::rc::Rc;
use term_ui::workspace_host::WorkspaceId;

use super::Window;
use super::frame::Look;

impl Window {
    /// Whether the tab bar of the window in front shows, and the titles of its tabs, the one in front marked `*`.
    pub(crate) fn tab_bar_state(&self) -> String {
        let Some(front) = self.front_frame() else {
            return "no window".into();
        };
        let shown = front.ns.tabGroup().is_some_and(|group| group.isTabBarVisible());
        format!(
            "{} {:?}",
            if shown { "shown" } else { "hidden" },
            self.titles(&self.group_tabs(&front))
        )
    }

    /// Every tab group of the window, on screen or off it: its workspace, frame, sidebar and tabs.
    pub(crate) fn groups_state(&self) -> Vec<String> {
        let mut seen: Vec<u64> = Vec::new();
        let mut lines = Vec::new();
        for workspace in self.workspaces() {
            for tab in workspace.tabs() {
                let Some(frame) = self.frame_of(tab.id).filter(|_| !seen.contains(&tab.id)) else {
                    continue;
                };
                let ids = if frame.is_shown() {
                    self.group_tabs(&frame)
                } else {
                    vec![tab.id]
                };
                seen.extend(&ids);
                let rect = frame.ns.frame();
                let Look { collapsed, .. } = frame.look();
                let width = ids
                    .iter()
                    .filter_map(|id| self.frame_of(*id))
                    .map(|own| format!("{:.0}", own.look().width))
                    .collect::<Vec<_>>()
                    .join("/");
                let shown_rows = frame
                    .sidebar
                    .selected()
                    .and_then(|id| self.workspaces.borrow().get(WorkspaceId::new(id)))
                    .map(|selected| selected.name());
                lines.push(format!(
                    "{} {} at {:.0},{:.0} {:.0}x{:.0} sidebar {} {} selects {:?}: {:?}",
                    workspace.name(),
                    if frame.is_shown() { "on screen" } else { "off screen" },
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                    if collapsed { "hidden" } else { "shown" },
                    width,
                    shown_rows,
                    self.titles(&ids)
                ));
            }
        }
        lines
    }

    /// The tabs of the front window's tab group in the order of its tab bar.
    pub(crate) fn front_group_tabs(&self) -> Vec<u64> {
        self.front_frame()
            .map(|front| self.group_tabs(&front))
            .unwrap_or_default()
    }

    /// How the sidebar of the window in front sits below its toolbar in full screen.
    pub(crate) fn full_screen_state(&self) -> String {
        self.front_frame().map_or_else(
            || "no window".into(),
            |front| super::full_screen::state(&front.ns, &front.sidebar_view),
        )
    }

    /// Slides the toolbar of the window in front down by `by` points, as the menu bar does in full screen.
    pub(crate) fn slide_toolbar(&self, by: f64) {
        if let Some(front) = self.front_frame() {
            super::full_screen::slide_toolbar(&front.ns, by);
        }
    }

    /// Drags the sidebar of the window in front to `width`, as the user does at its edge.
    pub(crate) fn set_sidebar_width(self: &Rc<Self>, width: f64) {
        if let Some(front) = self.front_frame() {
            front.apply(Look {
                collapsed: false,
                width,
            });
            self.sync_look(front.tab);
        }
    }

    /// Resizes the window in front to `width` by `height` points, keeping its top left, as the user does at its edge.
    pub(crate) fn set_size(&self, width: f64, height: f64) {
        let Some(front) = self.front_frame() else { return };
        let mut frame = front.ns.frame();
        frame.origin.y += frame.size.height - height;
        frame.size = objc2_foundation::NSSize::new(width, height);
        front.ns.setFrame_display(frame, true);
        self.sync_group(front.tab);
    }

    fn titles(&self, ids: &[u64]) -> Vec<String> {
        let front = self.selected_tab_id();
        ids.iter()
            .filter_map(|id| self.tab(*id))
            .map(|tab| {
                let mark = if front == Some(tab.id) { "*" } else { "" };
                let dot = self
                    .frame_of(tab.id)
                    .and_then(|frame| frame.ns.tab().accessoryView(self.mtm))
                    .map(|view| {
                        let words = objc2_app_kit::NSAccessibility::accessibilityLabel(&*view);
                        format!(" ●{}", words.map(|words| words.to_string()).unwrap_or_default())
                    })
                    .unwrap_or_default();
                format!("{}{mark}{dot}", tab.title())
            })
            .collect()
    }
}
