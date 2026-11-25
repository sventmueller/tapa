//! Closing panes, tabs, workspaces and the window, asking first while programs run in them (F213): the last pane
//! takes its tab and its tab's window; a workspace stays without tabs, and a workspace with tabs takes its place on
//! screen, the Terminal first; with the last tab of all no window shows, and the window sleeps with its workspaces
//! until the app is asked for one again. Only the user removes a workspace, never the Terminal.

use std::rc::Rc;
use term_ui::workspace_host::{TabId, TabKind, WorkspaceId};

use splat_i18n::{t, tf};

use super::Window;
use crate::state::app;
use crate::tab::{Tab, TabContent};

impl Window {
    /// What ⌘W does: closes the active pane, the tab with its last pane, and with the last tab what holds it.
    pub(crate) fn close_active(self: &Rc<Self>) {
        let Some(tab) = self.selected_tab() else { return };
        let panes = tab.terminal().map_or(1, |terminal| terminal.panes.len());
        if let Some(view) = tab.focused()
            && panes > 1
        {
            if self.confirm(&busy_of(&[view])) {
                self.close_session(&tab, view);
            }
            return;
        }
        self.close_tab(tab.id);
    }

    /// Closes the active pane, asking first when a program runs in it.
    pub(crate) fn close_pane(self: &Rc<Self>) {
        let Some(tab) = self.selected_tab() else { return };
        let Some(view) = tab.focused() else { return };
        if self.confirm(&busy_of(&[view])) {
            self.close_session(&tab, view);
        }
    }

    /// Closes the pane of session view `view`, asking first when a program runs in it; with the last pane its tab.
    pub(crate) fn close_view(self: &Rc<Self>, view: u64) {
        let Some(tab) = self.tab_of_view(view) else { return };
        if self.confirm(&busy_of(&[view])) {
            self.close_session(&tab, view);
        }
    }

    /// Closes tab `id` with all its panes, asking first when programs run in them.
    pub(crate) fn close_tab(self: &Rc<Self>, id: u64) {
        let Some(tab) = self
            .workspace_of_tab(id)
            .and_then(|workspace| workspace.tabs.borrow().get(TabId::new(id)))
        else {
            return;
        };
        if !self.editors_may_close(std::slice::from_ref(&tab), Retry::Tab(id)) {
            return;
        }
        if self.confirm(&tab.busy()) {
            self.remove_tab(&tab);
        }
    }

    /// Removes workspace `id` with its tabs, asking first when programs run in them; the next workspace takes its
    /// place on screen. The Terminal stays.
    pub(crate) fn close_workspace(self: &Rc<Self>, id: u64) {
        let Some(workspace) = self
            .workspaces
            .borrow()
            .get(WorkspaceId::new(id))
            .filter(|workspace| !workspace.scratch)
        else {
            return;
        };
        if !self.editors_may_close(&workspace.tabs(), Retry::Workspace(id)) {
            return;
        }
        if !self.confirm(&workspace.busy()) {
            return;
        }
        let shown = self.shown_frame_of(id);
        self.workspaces.borrow_mut().remove(WorkspaceId::new(id));
        self.next_in_place(shown);
        for tab in workspace.tabs() {
            end(id, &tab);
            self.forget_frame(tab.id, true);
        }
        crate::panels::workspace_closed(&workspace.handle());
        if let Some(remote) = workspace.remote_machine() {
            remote.forget();
        }
        self.refresh();
        crate::restore::save();
    }

    /// The close button and Close Window: closing the app's last window quits it, which keeps every workspace, tab
    /// and session for the next start and asks once while programs run; another window closes with all it shows
    /// after one question.
    pub(crate) fn close_window(self: &Rc<Self>) {
        if self.is_quick() {
            if self.may_close_window() {
                self.close_all();
            }
            return;
        }
        let others = app()
            .windows
            .borrow()
            .iter()
            .any(|window| window.id() != self.id() && !window.is_quick() && !window.is_asleep());
        if !others {
            #[cfg(feature = "selftest")]
            if term_base::unattended::is() {
                crate::quit::may_quit(self.mtm);
                return crate::selftest::menus::quits();
            }
            return objc2_app_kit::NSApplication::sharedApplication(self.mtm).terminate(None);
        }
        let tabs: Vec<Rc<Tab>> = self
            .workspaces()
            .iter()
            .flat_map(|workspace| workspace.tabs())
            .collect();
        if self.editors_may_close(&tabs, Retry::Window) && self.may_close_window() {
            self.close_all();
            crate::restore::save();
        }
    }

    /// Whether tab `id`'s window may close: its file is saved and no program runs in it, or the user agreed.
    pub(crate) fn may_close_tab(self: &Rc<Self>, id: u64) -> bool {
        self.tab(id).is_none_or(|tab| {
            self.editors_may_close(std::slice::from_ref(&tab), Retry::Tab(id)) && self.confirm(&tab.busy())
        })
    }

    /// Closes again what waited for a save the user asked for.
    fn close_again(self: &Rc<Self>, retry: Retry) {
        match retry {
            Retry::Tab(id) => self.close_tab(id),
            Retry::Workspace(id) => self.close_workspace(id),
            Retry::Window => self.close_window(),
        }
    }

    /// Whether the editors among `tabs` may close: saved, or the user gave up their changes. While a save the user
    /// asked for runs they stay, and `retry` closes again once it wrote the file.
    fn editors_may_close(self: &Rc<Self>, tabs: &[Rc<Tab>], retry: Retry) -> bool {
        for tab in tabs {
            if !tab.as_hosted().is_some_and(|hosted| hosted.kind() == TabKind::File) {
                continue;
            }
            let Some(workspace) = self.workspace_of_tab(tab.id) else {
                continue;
            };
            let window = self.id();
            let again: term_ui_editor::Again = Rc::new(move |saved| {
                if saved {
                    term_base::main_soon(move || {
                        if let Some(window) = crate::state::try_app().and_then(|app| app.window(window)) {
                            window.close_again(retry);
                        }
                    });
                }
            });
            let handle = workspace.handle();
            match term_ui_editor::may_close(&handle, TabId::new(tab.id), &again) {
                term_ui_editor::Answer::Yes => {}
                term_ui_editor::Answer::No | term_ui_editor::Answer::Later => return false,
            }
        }
        true
    }

    /// Whether the window may close: no program runs, or the user agreed.
    pub(crate) fn may_close_window(&self) -> bool {
        self.confirm(&self.busy())
    }

    /// Whether the user agrees to end `busy`, the programs that run; at once when none does or the settings do not ask.
    fn confirm(&self, busy: &[String]) -> bool {
        if busy.is_empty() || !app().config().window.confirm_close {
            return true;
        }
        if term_base::unattended::is() {
            #[cfg(feature = "selftest")]
            crate::selftest::menus::asked(busy);
            return true;
        }
        let message = t("Laufende Programme beenden?");
        let detail = tf(
            "Diese Programme laufen noch und werden beendet: {}",
            &[&busy.join(", ")],
        );
        crate::quit::ask(self.mtm, message, &detail, t("Beenden"))
    }

    pub(super) fn close_session(self: &Rc<Self>, tab: &Rc<Tab>, view: u64) {
        let Some(terminal) = tab.terminal() else {
            return self.remove_tab(tab);
        };
        let empty = terminal.panes.remove(view);
        terminal.sessions.borrow_mut().remove(&view);
        if let Some(view) = term_ui::find(view) {
            view.with_session(term_session::Session::close);
        }
        if empty {
            self.remove_tab(tab);
        } else {
            self.focus_active();
            self.refresh();
        }
    }

    /// Takes `tab` out of its workspace and closes its window (E20).
    pub(super) fn remove_tab(self: &Rc<Self>, tab: &Rc<Tab>) {
        self.take_out(tab, true);
    }

    /// Takes `tab` out as `remove_tab` does; its window closes already unless `close`.
    fn take_out(self: &Rc<Self>, tab: &Rc<Tab>, close: bool) {
        let Some(workspace) = self.workspace_of_tab(tab.id) else {
            return;
        };
        if self.is_quick() {
            return self.close_all();
        }
        let in_place = self.frame_of(tab.id).filter(|frame| frame.is_shown());
        end(workspace.id, tab);
        workspace.tabs.borrow_mut().remove(TabId::new(tab.id));
        if workspace.tabs.borrow().is_empty() {
            self.next_with_tabs(in_place);
        }
        self.forget_frame(tab.id, close);
        self.refresh();
    }

    /// A workspace with tabs, the Terminal first, comes to the front at `replaced`'s place, as the one `replaced`
    /// showed has none left; without one nothing shows.
    fn next_with_tabs(self: &Rc<Self>, replaced: Option<Rc<super::frame::TabWindow>>) {
        let Some(replaced) = replaced else { return };
        let next = self
            .workspaces()
            .into_iter()
            .find(|workspace| !workspace.tabs.borrow().is_empty());
        if let Some(next) = next {
            self.show_workspace(next.id, Some(replaced));
        }
    }

    /// The window on screen that shows workspace `id`, the one in front first.
    fn shown_frame_of(&self, id: u64) -> Option<Rc<super::frame::TabWindow>> {
        let frames = self.frames.borrow().clone();
        let shown: Vec<_> = frames
            .into_iter()
            .filter(|frame| frame.workspace == id && frame.is_shown())
            .collect();
        shown
            .iter()
            .find(|frame| frame.ns.isKeyWindow())
            .or(shown.first())
            .cloned()
    }

    /// The workspace selected now, after the one `replaced` showed closed, comes to the front at `replaced`'s place.
    fn next_in_place(self: &Rc<Self>, replaced: Option<Rc<super::frame::TabWindow>>) {
        let Some(next) = self.workspaces.borrow().selected_id().map(WorkspaceId::get) else {
            return;
        };
        if replaced.is_some() {
            self.show_workspace(next, replaced);
        }
    }

    /// Closes everything the window shows and forgets it.
    fn close_all(&self) {
        self.closing.set(true);
        for workspace in self.workspaces() {
            for tab in workspace.tabs() {
                end(workspace.id, &tab);
            }
            crate::panels::workspace_closed(&workspace.handle());
        }
        let frames = std::mem::take(&mut *self.frames.borrow_mut());
        for frame in frames {
            frame.ns.close();
        }
        if let Some(quick) = self.quick.borrow().as_ref() {
            quick.panel.close();
        }
        *self.workspaces.borrow_mut() = crate::workspace::Ordered::new();
        app().forget_window(self.id());
    }

    /// The user closed the quick terminal's panel, which `windowShouldClose:` agreed to.
    pub(crate) fn closed(&self) {
        if !self.closing.get() {
            self.close_all();
        }
    }

    /// Tab `id`'s window closes, as the user closed its tab or its tab group, which `windowShouldClose:` agreed to.
    pub(crate) fn tab_window_closed(self: &Rc<Self>, id: u64) {
        if self.closing.get() || self.frame_of(id).is_none() {
            return;
        }
        if let Some(tab) = self.tab(id) {
            self.take_out(&tab, false);
        } else {
            self.forget_frame(id, false);
        }
    }

    /// Where the user types in session `view`.
    pub(crate) fn context(&self, view: u64) -> Option<term_suggest::Context> {
        self.tab_of_view(view)?.context(view)
    }

    /// Every program that runs in the window, for quitting the app.
    pub(crate) fn busy(&self) -> Vec<String> {
        self.workspaces()
            .iter()
            .flat_map(|workspace| workspace.busy())
            .collect()
    }
}

/// What closes again once the saves a closing waited for are over.
#[derive(Debug, Clone, Copy)]
enum Retry {
    Tab(u64),
    Workspace(u64),
    /// The whole window.
    Window,
}

/// Ends `tab` of workspace `workspace` as it closes; an editor first lets go of its document.
fn end(workspace: u64, tab: &Tab) {
    if tab.as_hosted().is_some_and(|hosted| hosted.kind() == TabKind::File) {
        let handle = term_ui::workspace_host::Workspace::new(term_ui::workspace_host::WorkspaceId::new(workspace));
        term_ui_editor::closing(&handle, TabId::new(tab.id));
    }
    tab.close();
}

/// The programs running in the sessions `views`.
fn busy_of(views: &[u64]) -> Vec<String> {
    views
        .iter()
        .filter_map(|id| term_ui::find(*id))
        .filter_map(|view| view.with_session(term_session::Session::foreground))
        .filter_map(|front| match front {
            term_session::Foreground::Agent(agent) => Some(format!("{agent:?}").to_lowercase()),
            term_session::Foreground::Remote(name) | term_session::Foreground::Other(name) => Some(name),
            _ => None,
        })
        .collect()
}
