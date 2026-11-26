//! A window stored as the app quits and opened again at the next start (F71), with its workspaces, their tabs as
//! windows of each workspace's tab group, the windows dragged out of a group, and the sidebar.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;
use term_ui::workspace_host::{TabId, WorkspaceId};

use objc2_foundation::{NSPoint, NSRect, NSSize};
use term_panes::layout::{Layout, Node, WindowTab};
use term_ui_panes::Panes;

use super::frame::{Look, TabWindow};
use super::{Window, geometry, session};
use crate::restore::{
    Budget, SavedAgent, SavedBrowser, SavedFile, SavedGroup, SavedSession, SavedWindow, SavedWorkspace, put_rows,
};
use crate::state::{app, try_app};
use crate::tab::Tab;
use crate::workspace::Workspace;

#[derive(Default)]
struct RestoredShells {
    waiting: VecDeque<(u64, u64)>,
    scheduled: bool,
}

pub(super) struct RestoredFrame {
    workspace: u64,
    frame: Option<NSRect>,
    look: Look,
}

thread_local! {
    static RESTORED_SHELLS: RefCell<RestoredShells> = RefCell::default();
}

/// Leave time for input and drawing between shell starts.
const SHELL_START_GAP: Duration = Duration::from_millis(4);

fn start_next_shell_tab() {
    if crate::quit::left() {
        return;
    }
    let next = RESTORED_SHELLS.with(|shells| {
        let mut shells = shells.borrow_mut();
        let next = shells.waiting.pop_front();
        shells.scheduled = next.is_some();
        next
    });
    if let Some((window, tab)) = next {
        if let Some(window) = try_app().and_then(|app| app.window(window))
            && let Some(tab) = window.tab(tab)
        {
            window.attach_restored_frame(&tab);
            window.start_sessions(&tab);
        }
        term_base::main_after(SHELL_START_GAP, start_next_shell_tab);
    }
}

impl Window {
    /// The window as it is now, with up to `lines` rows of history per session, without its Terminal; none for a
    /// window that neither shows nor holds a workspace the user keeps.
    pub(crate) fn snapshot(&self, lines: usize, budget: &mut Budget) -> Option<SavedWindow> {
        self.sync_order();
        let budget = RefCell::new(budget);
        let mut workspaces = Vec::new();
        let mut selected = None;
        for workspace in self.workspaces() {
            let Some((mut stored, saved)) = snapshot_workspace(&workspace, lines, &budget) else {
                continue;
            };
            stored.separate = self.separate_groups(&workspace, &saved);
            if self.selected_workspace().is_some_and(|own| own.id == workspace.id) {
                selected = Some(workspaces.len());
            }
            workspaces.push(stored);
        }
        let front = self.front_frame();
        if workspaces.is_empty() && front.is_none() {
            return None;
        }
        let look = front.as_ref().map(|front| front.look());
        Some(SavedWindow {
            frame: front.map(|front| rect(front.ns.frame())),
            workspaces,
            selected,
            sidebar_hidden: look.is_some_and(|look| look.collapsed),
            sidebar_width: look.map(|look| look.width),
            without_scratch: true,
        })
    }

    /// The tab groups on screen of `workspace` but the one with its tab in front, by the places of their tabs among
    /// `saved`, the ids of the tabs stored in their order.
    fn separate_groups(&self, workspace: &Workspace, saved: &[u64]) -> Vec<SavedGroup> {
        let front = workspace.tabs.borrow().selected_id().map(TabId::get);
        let mut groups: Vec<SavedGroup> = Vec::new();
        let mut seen: Vec<u64> = Vec::new();
        for tab in workspace.tabs() {
            let Some(frame) = self.frame_of(tab.id).filter(|frame| frame.is_shown()) else {
                continue;
            };
            if seen.contains(&tab.id) {
                continue;
            }
            let ids = self.group_tabs(&frame);
            seen.extend(&ids);
            if front.is_some_and(|front| ids.contains(&front)) {
                continue;
            }
            let tabs: Vec<usize> = ids
                .iter()
                .filter_map(|id| saved.iter().position(|own| own == id))
                .collect();
            if !tabs.is_empty() {
                groups.push(SavedGroup {
                    tabs,
                    frame: rect(frame.ns.frame()),
                });
            }
        }
        groups
    }

    /// The tabs of `frame`'s tab group in the order of its tab bar.
    pub(super) fn group_tabs(&self, frame: &TabWindow) -> Vec<u64> {
        let frames = self.frames.borrow();
        frame
            .group()
            .iter()
            .filter_map(|window| frames.iter().find(|own| std::ptr::eq(&*own.ns, &**window)))
            .map(|own| own.tab)
            .collect()
    }

    /// Opens `saved` again: a new Terminal, then its workspaces, tabs and panes with shells in their folders, each tab
    /// in its window, at its frame while that is on a screen.
    pub(crate) fn reopen(saved: &SavedWindow) -> Rc<Window> {
        let window = Window::empty();
        window.restoring.set(true);
        let scratch = window.add_scratch();
        let look = Look {
            collapsed: saved.sidebar_hidden,
            width: saved
                .sidebar_width
                .unwrap_or(geometry::SIDEBAR_WIDTH)
                .clamp(geometry::SIDEBAR_MIN, geometry::SIDEBAR_MAX),
        };
        let frame = saved
            .frame
            .map(ns_rect)
            .filter(|frame| geometry::on_a_screen(window.mtm, *frame));
        let place = std::cell::Cell::new(frame);
        let mut selected = scratch.id;
        let mut with_agents = Vec::new();
        for (index, stored) in saved.workspaces.iter().enumerate() {
            let was_scratch =
                index == 0 && !saved.without_scratch && stored.name.is_none() && stored.base == scratch.base;
            if was_scratch {
                continue;
            }
            let hosted = !stored.agents.is_empty() || !stored.browsers.is_empty() || !stored.files.is_empty();
            let eager =
                saved.selected == Some(index) || hosted || !stored.separate.is_empty() || stored.remote.is_some();
            let workspace = window.reopen_workspace(stored, &place, look, eager);
            if hosted {
                with_agents.push((workspace.clone(), stored));
            }
            if saved.selected == Some(index) {
                selected = workspace.id;
            }
        }
        let hosted = |id: u64| with_agents.iter().any(|(workspace, _)| workspace.id == id);
        if let Some(front) = window.workspaces.borrow().get(WorkspaceId::new(selected))
            && front.tabs.borrow().is_empty()
            && !hosted(front.id)
        {
            // The window in front opens where it was, also for a workspace whose tabs were all closed.
            let tab = window.make_tab(front.base.clone(), None);
            front.tabs.borrow_mut().push(tab.clone());
            front.tabs.borrow_mut().select(TabId::new(tab.id));
            window.attach(&tab, front.id, None, place.get(), Some(look));
        }
        for (workspace, stored) in &with_agents {
            window.reopen_hosted(workspace, stored);
        }
        for workspace in window.workspaces().into_iter().filter(|workspace| !workspace.scratch) {
            if let Some(stored) = saved.workspaces.iter().find(|stored| stored.base == workspace.base) {
                window.reopen_separate(&workspace, &stored.separate);
            }
        }
        window.restoring.set(false);
        window.queue_restored_shells();
        window.workspaces.borrow_mut().select(WorkspaceId::new(selected));
        window.show_workspace(selected, None);
        window.focus_active();
        window
    }

    /// Builds the native window of a restored terminal tab the first time it needs one.
    pub(super) fn attach_restored_frame(&self, tab: &Tab) {
        let stored = self.restored_frames.borrow_mut().remove(&tab.id);
        if let Some(RestoredFrame { workspace, frame, look }) = stored {
            self.attach(tab, workspace, None, frame, Some(look));
        }
    }

    fn queue_restored_shells(&self) {
        let mut tabs = self.tabs();
        tabs.retain(|tab| tab.terminal().is_some());
        // Detached windows already on screen should fill before the tabs behind them.
        tabs.sort_by_cached_key(|tab| !self.frame_of(tab.id).is_some_and(|frame| frame.ns.isVisible()));
        let schedule = RESTORED_SHELLS.with(|shells| {
            let mut shells = shells.borrow_mut();
            shells.waiting.extend(tabs.iter().map(|tab| (self.id(), tab.id)));
            if shells.scheduled || shells.waiting.is_empty() {
                return false;
            }
            shells.scheduled = true;
            true
        });
        if schedule {
            term_base::main_thread::after_frame(|| term_base::main_after(SHELL_START_GAP, start_next_shell_tab));
        }
    }

    /// A restored tab taking the keyboard starts before the next key can arrive.
    pub(super) fn start_restored_shells(&self, tab: &Tab) {
        let waiting = RESTORED_SHELLS.with(|shells| {
            let mut shells = shells.borrow_mut();
            let position = shells.waiting.iter().position(|key| *key == (self.id(), tab.id));
            position.and_then(|position| shells.waiting.remove(position)).is_some()
        });
        if waiting {
            self.start_sessions(tab);
        }
    }

    /// Adds the workspace and its tabs at `place`, building native windows now for groups needed at launch.
    fn reopen_workspace(
        &self,
        stored: &SavedWorkspace,
        place: &std::cell::Cell<Option<NSRect>>,
        look: Look,
        eager: bool,
    ) -> Rc<Workspace> {
        let workspace = match &stored.remote {
            Some(target) => {
                let remote = crate::remote::Remote::at(target.clone(), stored.base.clone());
                remote.connect();
                let workspace = Rc::new(Workspace::remote(app().next_id(), remote, stored.name.clone()));
                self.workspaces.borrow_mut().insert_after_selected(workspace.clone());
                workspace
            }
            None => self.add_workspace(stored.base.clone(), stored.name.clone()),
        };
        workspace.set_trusted(stored.trusted);
        *workspace.panels.borrow_mut() = stored.panels;
        self.workspaces.borrow_mut().select(WorkspaceId::new(workspace.id));
        for (at, tab) in stored.layout.tabs.iter().enumerate() {
            let Some(tab) = self.reopen_tab(&tab.root) else {
                continue;
            };
            {
                let mut tabs = workspace.tabs.borrow_mut();
                tabs.push(tab.clone());
                if stored.layout.selected == Some(at) || tabs.selected_id().is_none() {
                    tabs.select(TabId::new(tab.id));
                }
            }
            if eager {
                let frame = self.attach(&tab, workspace.id, None, place.get(), Some(look));
                if place.get().is_none() {
                    place.set(Some(frame.ns.frame()));
                }
            } else {
                self.restored_frames.borrow_mut().insert(
                    tab.id,
                    RestoredFrame {
                        workspace: workspace.id,
                        frame: place.get(),
                        look,
                    },
                );
            }
        }
        workspace
    }

    /// Opens the agent, browser and editor tabs of `workspace` again at their places, resuming the agents' sessions,
    /// loading each page once its tab first shows and reading the files; a workspace left without a tab, as nothing
    /// came back, gets a terminal.
    fn reopen_hosted(self: &Rc<Self>, workspace: &Rc<Workspace>, stored: &SavedWorkspace) {
        let front = workspace.tabs.borrow().selected_id().map(TabId::get);
        let mut selected = front;
        let mut kept: Vec<(usize, bool, Hosted)> = stored
            .agents
            .iter()
            .map(|agent| (agent.at, agent.selected, Hosted::Agent(&agent.session)))
            .chain(
                stored
                    .browsers
                    .iter()
                    .map(|browser| (browser.at, browser.selected, Hosted::Browser(&browser.url))),
            )
            .chain(
                stored
                    .files
                    .iter()
                    .map(|file| (file.at, file.selected, Hosted::File(&file.path))),
            )
            .collect();
        kept.sort_by_key(|(at, _, _)| *at);
        for (at, was_selected, hosted) in kept {
            self.workspaces.borrow_mut().select(WorkspaceId::new(workspace.id));
            let reopened = match hosted {
                Hosted::Agent(session) => crate::agent::reopen(workspace.id, session),
                Hosted::Browser(url) => crate::browser::reopen(workspace.id, url),
                Hosted::File(path) => crate::editor_tab::reopen(self, workspace.id, path),
            };
            let Some(id) = reopened else {
                continue;
            };
            workspace.tabs.borrow_mut().move_to(TabId::new(id), at);
            if was_selected || selected.is_none() {
                selected = Some(id);
            }
        }
        if workspace.tabs.borrow().is_empty() {
            let tab = self.make_tab(workspace.base.clone(), None);
            workspace.tabs.borrow_mut().push(tab.clone());
            selected = Some(tab.id);
            self.attach(&tab, workspace.id, None, None, None);
        }
        if let Some(id) = selected {
            workspace.select_tab(TabId::new(id));
        }
    }

    /// Shows the windows dragged out of `workspace`'s tab group again, each group at its frame.
    fn reopen_separate(&self, workspace: &Workspace, groups: &[crate::restore::SavedGroup]) {
        let tabs = workspace.tabs();
        for group in groups {
            let members: Vec<Rc<Tab>> = group.tabs.iter().filter_map(|at| tabs.get(*at).cloned()).collect();
            let frame = ns_rect(group.frame);
            if members.is_empty() || !geometry::on_a_screen(self.mtm, frame) {
                continue;
            }
            for tab in &members {
                if let Some(own) = self.frame_of(tab.id) {
                    own.ns.setFrame_display(frame, false);
                }
            }
            self.bring_back(&members, None, None);
        }
    }

    fn reopen_tab(&self, root: &Node<SavedSession>) -> Option<Rc<Tab>> {
        let mut opened: Vec<(u64, &SavedSession)> = Vec::new();
        let panes = Panes::from_layout(self.mtm, root, |saved| {
            let folder = if saved.folder.is_dir() {
                saved.folder.clone()
            } else {
                let config = app().config();
                session::start_folder(&config, None, false)
            };
            let (view, view_root) = session::make(self.mtm, self.id(), folder, None);
            put_rows(view.id(), saved.rows.clone());
            if let Some(on) = saved.field {
                view.set_field_enabled(on);
            }
            opened.push((view.id(), saved));
            (view.id(), view_root)
        })?;
        let tab = Rc::new(Tab::with_panes(app().next_id(), panes));
        for (view, saved) in opened {
            let folder = term_ui::find(view).map_or_else(|| saved.folder.clone(), |view| view.cwd());
            tab.add_session(view, folder);
            let Some(terminal) = tab.terminal() else { continue };
            if let Some(session) = terminal.sessions.borrow_mut().get_mut(&view) {
                session.title.clone_from(&saved.title);
            }
            if saved.focused {
                terminal.panes.focus(view);
            }
        }
        Some(tab)
    }
}

/// What is stored of `workspace`, also without tabs: its tabs with their panes, sessions and history; and the ids of
/// the tabs stored, in their order. Nothing of the Terminal.
fn snapshot_workspace(
    workspace: &Workspace,
    lines: usize,
    budget: &RefCell<&mut Budget>,
) -> Option<(SavedWorkspace, Vec<u64>)> {
    if workspace.scratch {
        return None;
    }
    let mut saved = Vec::new();
    let mut tabs = Vec::new();
    let mut agents = Vec::new();
    let mut browsers = Vec::new();
    let mut files = Vec::new();
    let mut selected = None;
    let front = workspace.tabs.borrow().selected_id().map(TabId::get);
    for tab in workspace.tabs() {
        if let Some(session) = tab.as_hosted().and_then(|hosted| crate::agent::kept(&hosted.key()?)) {
            agents.push(SavedAgent {
                at: saved.len(),
                session,
                selected: front == Some(tab.id),
            });
            saved.push(tab.id);
            continue;
        }
        if let Some(path) = crate::editor_tab::kept(workspace.id, &tab) {
            files.push(SavedFile {
                at: saved.len(),
                path,
                selected: front == Some(tab.id),
            });
            saved.push(tab.id);
            continue;
        }
        if let Some(url) = crate::browser::kept(workspace.id, &tab) {
            browsers.push(SavedBrowser {
                at: saved.len(),
                url,
                selected: front == Some(tab.id),
            });
            saved.push(tab.id);
            continue;
        }
        let Some(root) = snapshot_tab(&tab, lines, budget) else {
            continue;
        };
        if front == Some(tab.id) {
            selected = Some(tabs.len());
        }
        tabs.push(WindowTab { root });
        saved.push(tab.id);
    }
    let stored = SavedWorkspace {
        base: workspace.base.clone(),
        name: workspace.custom_name(),
        layout: Layout { tabs, selected },
        trusted: workspace.is_trusted(),
        agents,
        browsers,
        files,
        panels: *workspace.panels.borrow(),
        separate: Vec::new(),
        remote: workspace.remote_machine().map(|remote| remote.target()),
    };
    Some((stored, saved))
}

fn snapshot_tab(tab: &Tab, lines: usize, budget: &RefCell<&mut Budget>) -> Option<Node<SavedSession>> {
    let focused = tab.focused();
    let terminal = tab.terminal()?;
    let sessions = terminal.sessions.borrow();
    terminal.panes.to_layout(&|view| {
        let known = sessions.get(&view);
        let found = term_ui::find(view);
        let folder = known
            .map(|session| session.folder.clone())
            .or_else(|| found.as_ref().map(|view| view.cwd()))?;
        let rows = if lines > 0 {
            let mut budget = budget.borrow_mut();
            let bytes = budget.available();
            let mut rows = found
                .as_ref()
                .and_then(|found| found.with_session(|session| session.restore_rows(lines, bytes)))
                .unwrap_or_else(|| crate::restore::pending_rows(view, lines, bytes));
            budget.spent(&rows);
            term_base::redact::redact_lines(&mut rows);
            rows
        } else {
            Vec::new()
        };
        Some(SavedSession {
            folder,
            title: known.and_then(|session| {
                session
                    .title
                    .as_deref()
                    .map(|title| term_base::redact::redact(title).into_owned())
            }),
            focused: focused == Some(view),
            rows,
            field: found.as_ref().map(|view| view.field_enabled()),
        })
    })
}

fn rect(frame: NSRect) -> [f64; 4] {
    [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height]
}

fn ns_rect([x, y, width, height]: [f64; 4]) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

/// A tab of a UI crate the restore opens again.
enum Hosted<'a> {
    /// An agent tab by its session.
    Agent(&'a str),
    /// A browser tab by its page's address.
    Browser(&'a str),
    /// An editor tab by its file.
    File(&'a std::path::Path),
}
