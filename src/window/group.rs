//! The native tab groups of a window's workspaces (E20, F39): each tab's window joins the group of its workspace,
//! picking another workspace swaps the group on screen for that workspace's at the same place, Merge All Windows
//! gathers the windows of one workspace, and the order of a tab bar is the order of the workspace's tabs.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use term_ui::workspace_host::{TabId, TabKind, WorkspaceId};

use objc2_app_kit::{NSWindow, NSWindowAnimationBehavior, NSWindowOrderingMode, NSWindowTabbingMode};
use objc2_foundation::NSRect;

use super::Window;
use super::frame::{Look, TabWindow};
use crate::tab::{Tab, TabContent};

impl Window {
    /// Makes the window of `tab`, of workspace `workspace`: in the tab group of tab `anchor`'s window right after it
    /// when that is on screen, else off screen at `frame`, the anchor's or the front window's place; its sidebar as
    /// `look`, the anchor's or the front window's.
    pub(super) fn attach(
        &self,
        tab: &Tab,
        workspace: u64,
        anchor: Option<u64>,
        frame: Option<NSRect>,
        look: Option<Look>,
    ) -> Rc<TabWindow> {
        let anchor = anchor.and_then(|id| self.frame_of(id));
        let near = anchor.clone().or_else(|| self.front_frame());
        let frame = frame.or_else(|| near.as_ref().map(|near| near.ns.frame()));
        let look = look
            .or_else(|| near.as_ref().map(|near| near.look()))
            .unwrap_or_else(|| super::last::look(&crate::state::app().config()));
        let own = TabWindow::new(self, (tab.id, workspace), &tab.view(), frame, look);
        // A terminal's text starts right below the tab bar; the other kinds bring a heading with its own line.
        if tab.terminal().is_some() {
            own.soften_edge();
        }
        own.ns.setExcludedFromWindowsMenu(true);
        self.frames.borrow_mut().push(own.clone());
        if let Some(anchor) = anchor.filter(|anchor| anchor.is_shown()) {
            anchor.ns.addTabbedWindow_ordered(&own.ns, NSWindowOrderingMode::Above);
            own.ns.setExcludedFromWindowsMenu(false);
        }
        own
    }

    /// Brings workspace `id` forward with its tab in front. When that tab's window is on screen it comes to the
    /// front; else the workspace's windows off screen come back as one tab group in place of `replacing`'s group,
    /// which leaves the screen. A workspace without tabs gets a terminal first.
    pub(super) fn show_workspace(self: &Rc<Self>, id: u64, replacing: Option<Rc<TabWindow>>) {
        let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(id)) else {
            return;
        };
        let replacing = replacing.filter(|frame| frame.workspace != id && frame.is_shown());
        let empty = workspace.tabs.borrow().is_empty();
        let started = empty.then(|| {
            let tab = self.make_tab(workspace.base.clone(), None);
            workspace.tabs.borrow_mut().push(tab.clone());
            let (frame, look) = replacing.as_ref().map(|r| (r.ns.frame(), r.look())).unzip();
            self.attach(&tab, id, None, frame, look);
            tab
        });
        let Some(front) = workspace.selected_tab().or_else(|| workspace.tabs().first().cloned()) else {
            return;
        };
        workspace.select_tab(TabId::new(front.id));
        self.workspaces.borrow_mut().select(WorkspaceId::new(id));
        let Some(front_frame) = self.frame_of(front.id) else {
            return;
        };
        let swapping = !front_frame.is_shown();
        {
            let _arranging = term_ui::kit::FlagGuard::raise(&self.arranging);
            if swapping {
                super::frame::building(|| self.bring_back(&workspace.tabs(), Some(front.id), replacing.as_deref()));
            }
            front_frame.ns.makeKeyAndOrderFront(None);
            if let Some(replacing) = replacing.filter(|_| swapping) {
                self.sync_group_order(&replacing);
                self.take_away(&replacing);
            }
        }
        self.tab_became_key(front.id);
        if let Some(tab) = started {
            self.start_sessions(&tab);
        }
    }

    /// Shows the windows of `tabs` that are off screen as one tab group in their order, at the place of `replacing`
    /// and behind it, with its sidebar and tab `front`'s window, else the first, selected: the group builds up out of
    /// sight around that window and comes forward whole.
    pub(super) fn bring_back(&self, tabs: &[Rc<Tab>], front: Option<u64>, replacing: Option<&TabWindow>) {
        let hidden: Vec<Rc<TabWindow>> = tabs
            .iter()
            .filter_map(|tab| self.frame_of(tab.id))
            .filter(|frame| !frame.is_shown())
            .collect();
        let place = replacing.map(|replacing| (replacing.ns.frame(), replacing.look()));
        for frame in &hidden {
            if let Some((rect, look)) = place {
                if frame.ns.frame() != rect {
                    frame.ns.setFrame_display(rect, false);
                }
                frame.apply(look);
            }
            frame.ns.setExcludedFromWindowsMenu(false);
        }
        let at = front
            .and_then(|front| hidden.iter().position(|frame| frame.tab == front))
            .unwrap_or_default();
        let Some(shown) = hidden.get(at) else {
            return;
        };
        // A window ordered in grows from a smaller size, as a new document window does, and a tab bar that gains tabs
        // slides them into place.
        let animations: Vec<_> = hidden.iter().map(|frame| frame.ns.animationBehavior()).collect();
        for frame in &hidden {
            frame.ns.setAnimationBehavior(NSWindowAnimationBehavior::None);
        }
        still(|| {
            // Grouped before any of them shows, the tab bar has its tabs in place as the group comes forward; grouped
            // on screen, it slides them there. The window a group starts from stays its selected one.
            for frame in &hidden[..at] {
                shown.ns.addTabbedWindow_ordered(&frame.ns, NSWindowOrderingMode::Below);
            }
            let mut last: &NSWindow = &shown.ns;
            for frame in &hidden[at + 1..] {
                last.addTabbedWindow_ordered(&frame.ns, NSWindowOrderingMode::Above);
                last = &frame.ns;
            }
            // AppKit would add a window ordered in as a tab to a shown window of its tabbing identifier, such as the
            // rest of a workspace one of its tabs was moved out of.
            let mode = shown.ns.tabbingMode();
            shown.ns.setTabbingMode(NSWindowTabbingMode::Disallowed);
            match replacing {
                Some(replacing) => shown
                    .ns
                    .orderWindow_relativeTo(NSWindowOrderingMode::Below, replacing.ns.windowNumber()),
                None => shown.ns.orderFront(None),
            }
            shown.ns.setTabbingMode(mode);
            shown.show_tab_bar();
        });
        for (frame, animation) in hidden.iter().zip(animations) {
            frame.ns.setAnimationBehavior(animation);
        }
    }

    /// Takes `frame`'s tab group off the screen, keeping its windows for when their workspace shows again.
    fn take_away(&self, frame: &TabWindow) {
        let group = frame.group();
        let frames = self.frames.borrow().clone();
        for window in group {
            if let Some(own) = frames.iter().find(|own| std::ptr::eq(&*own.ns, &*window)) {
                own.hide();
            }
        }
    }

    /// The user picked workspace `workspace` in the sidebar of tab `from`'s window, which it replaces on screen.
    pub(crate) fn pick_workspace(self: &Rc<Self>, from: u64, workspace: u64) {
        let replacing = self.frame_of(from);
        self.show_workspace(workspace, replacing);
    }

    /// Gathers every window of tab `tab`'s workspace as tabs of its window's group in the workspace's order, as Merge
    /// All Windows does; the other workspaces' windows keep theirs.
    pub(crate) fn merge_all(&self, tab: u64) {
        let Some(target) = self.frame_of(tab) else { return };
        self.sync_order();
        let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(target.workspace)) else {
            return;
        };
        let look = target.look();
        let mut before: Vec<Rc<TabWindow>> = Vec::new();
        let mut last: Option<Rc<TabWindow>> = None;
        for frame in workspace.tabs().iter().filter_map(|tab| self.frame_of(tab.id)) {
            if target.groups_with(&frame.ns) {
                for early in before.drain(..) {
                    frame.ns.addTabbedWindow_ordered(&early.ns, NSWindowOrderingMode::Below);
                }
            } else {
                frame.apply(look);
                frame.ns.setExcludedFromWindowsMenu(false);
                match &last {
                    Some(last) => last.ns.addTabbedWindow_ordered(&frame.ns, NSWindowOrderingMode::Above),
                    None => {
                        before.push(frame);
                        continue;
                    }
                }
            }
            last = Some(frame);
        }
        target.ns.makeKeyAndOrderFront(None);
        self.sync_group_order(&target);
        self.refresh();
    }

    /// AppKit may already have notified us synchronously while bringing the window forward.
    pub(super) fn ensure_tab_key(&self, tab: u64) {
        if self.last_key.get() != Some(tab) || self.selected_tab_id() != Some(tab) {
            self.tab_became_key(tab);
        }
    }

    /// Tab `tab`'s window took the keyboard: its tab and workspace are in front, its sidebar shows as the one of its
    /// group before it did, and what it needed attention for has had it.
    pub(crate) fn tab_became_key(&self, tab: u64) {
        if self.arranging.get() {
            return;
        }
        let Some(workspace) = self.workspace_of_tab(tab) else {
            return;
        };
        if let Some(frame) = self.frame_of(tab) {
            frame.show_tab_bar();
            self.sync_group_order(&frame);
        }
        self.workspaces.borrow_mut().select(WorkspaceId::new(workspace.id));
        workspace.select_tab(TabId::new(tab));
        let before = self.last_key.replace(Some(tab)).filter(|before| *before != tab);
        if let (Some(before), Some(own)) = (before.and_then(|id| self.frame_of(id)), self.frame_of(tab))
            && own.groups_with(&before.ns)
        {
            own.apply(before.look());
        }
        if let Some(tab) = workspace.tabs.borrow().get(TabId::new(tab)) {
            tab.seen();
            if tab.as_hosted().is_some_and(|hosted| hosted.kind() == TabKind::Agent) {
                crate::agent::shown(workspace.id);
            }
        }
        self.refresh();
    }

    /// Gives the other windows of tab `tab`'s group its frame and sidebar, as the user resized it.
    pub(crate) fn sync_group(&self, tab: u64) {
        let Some(own) = self.frame_of(tab) else { return };
        let rect = own.ns.frame();
        for other in self.group_others(&own) {
            if other.ns.frame() != rect {
                other.ns.setFrame_display(rect, false);
            }
        }
        self.sync_look(tab);
    }

    /// Gives the other windows of tab `tab`'s group its sidebar; new windows take its size and sidebar width.
    pub(crate) fn sync_look(&self, tab: u64) {
        let Some(own) = self.frame_of(tab) else { return };
        let look = own.look();
        for other in self.group_others(&own) {
            other.apply(look);
        }
        super::last::remember(&own, look);
    }

    /// The app's other windows in `own`'s tab group.
    fn group_others(&self, own: &TabWindow) -> Vec<Rc<TabWindow>> {
        let frames = self.frames.borrow().clone();
        own.group()
            .iter()
            .filter(|window| !std::ptr::eq(&***window, &*own.ns))
            .filter_map(|window| frames.iter().find(|frame| std::ptr::eq(&*frame.ns, &**window)).cloned())
            .collect()
    }

    /// Takes the order of every tab bar into the workspaces' tabs, as the user may have dragged tabs around.
    pub(super) fn sync_order(&self) {
        let frames = self.frames.borrow().clone();
        let by_window: HashMap<_, _> = frames
            .iter()
            .map(|frame| (std::ptr::from_ref(&*frame.ns), frame.tab))
            .collect();
        let mut seen = HashSet::new();
        for frame in &frames {
            if seen.contains(&frame.tab) {
                continue;
            }
            let ids = Self::native_order(frame, &by_window);
            seen.extend(ids.iter().map(|id| id.get()));
            self.record_group_order(frame, &ids);
        }
    }

    pub(super) fn sync_group_order(&self, frame: &TabWindow) {
        let by_window = self
            .frames
            .borrow()
            .iter()
            .map(|frame| (std::ptr::from_ref(&*frame.ns), frame.tab))
            .collect();
        self.record_group_order(frame, &Self::native_order(frame, &by_window));
    }

    fn native_order(frame: &TabWindow, by_window: &HashMap<*const NSWindow, u64>) -> Vec<TabId> {
        frame
            .group()
            .iter()
            .filter_map(|window| by_window.get(&std::ptr::from_ref(&**window)).copied())
            .map(TabId::new)
            .collect()
    }

    fn record_group_order(&self, frame: &TabWindow, ids: &[TabId]) {
        if ids.len() > 1
            && let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(frame.workspace))
        {
            workspace.tabs.borrow_mut().arrange(ids);
        }
    }

    /// Forgets the window of tab `id`, closing it unless it closes already.
    pub(super) fn forget_frame(&self, id: u64, close: bool) {
        let frame = {
            let mut frames = self.frames.borrow_mut();
            let at = frames.iter().position(|frame| frame.tab == id);
            at.map(|at| frames.remove(at))
        };
        if self.last_key.get() == Some(id) {
            self.last_key.set(None);
        }
        if let Some(frame) = frame
            && close
        {
            frame.ns.close();
        }
    }
}

/// Runs `change` to tab groups without animations. The run loop presents the final group.
fn still(change: impl FnOnce()) {
    objc2_quartz_core::CATransaction::begin();
    objc2_quartz_core::CATransaction::setDisableActions(true);
    objc2_app_kit::NSAnimationContext::beginGrouping();
    let context = objc2_app_kit::NSAnimationContext::currentContext();
    context.setDuration(0.0);
    context.setAllowsImplicitAnimation(false);
    change();
    objc2_app_kit::NSAnimationContext::endGrouping();
    objc2_quartz_core::CATransaction::commit();
}
