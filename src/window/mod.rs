//! A window of the app (E20): its workspaces in the sidebar and each tab of a workspace as a window of the
//! workspace's native tab group, as in Finder, every one with its own sidebar of the same workspaces. Picking another
//! workspace swaps the tab group in place; the panes of a tab fill its window (E12).

mod closing;
mod delegate;
mod frame;
mod full_screen;
mod geometry;
mod group;
mod hosted;
mod last;
mod panels;
mod plus_menu;
#[cfg(feature = "selftest")]
mod probe;
mod quick;
mod restore;
mod session;
pub(crate) mod spoken_tab;
mod tabs;
mod title;
mod toolbar;
mod workspaces;

pub(crate) use last::store as store_last_window;
pub(crate) use session::{start_folder, toggle_field};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use term_ui::workspace_host::{TabId, WorkspaceId};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSLayoutConstraint, NSView, NSWindow};
use objc2_foundation::NSArray;
use term_session::Program;
use term_ui::Change;
use term_ui::kit::WindowId;

use crate::state::app;
use crate::tab::{Tab, TabContent};
use crate::workspace::{Ordered, Workspace};
use delegate::Delegate;
use frame::{Look, TabWindow};

/// The quick terminal's panel, one workspace with one tab and no lists (F45).
struct Quick {
    panel: Retained<NSWindow>,
    _delegate: Retained<Delegate>,
}

/// A window with its workspaces, shown as the tab windows of the one in front.
pub(crate) struct Window {
    id: WindowId,
    mtm: MainThreadMarker,
    workspaces: RefCell<Ordered<Workspace>>,
    /// The window of every tab of every workspace, on screen or kept off it while another workspace shows.
    frames: RefCell<Vec<Rc<TabWindow>>>,
    quick: RefCell<Option<Quick>>,
    /// Set while the window closes, so that tabs closing with it do not close it again.
    closing: Cell<bool>,
    /// Set while a tab group swaps or comes back, so that its windows taking the keyboard in passing pick nothing.
    arranging: Cell<bool>,
    /// Set while the window opens again from what was stored, so that tabs coming back stay off screen until all are.
    restoring: Cell<bool>,
    /// The tab whose window had the keyboard last, whose sidebar the next tab of its group takes over.
    last_key: Cell<Option<u64>>,
    /// The sidebar setting the window last followed, so that reading the settings again changes the sidebar only
    /// when that setting changed.
    sidebar_setting: Cell<bool>,
}

impl Window {
    /// Shows a window with a tab in its Terminal in `folder`, running `program` or the profile's shell: the window
    /// whose tabs were all closed, with the workspaces it keeps, else a new one.
    pub(crate) fn open(folder: Option<PathBuf>, program: Option<Program>) -> Rc<Window> {
        let config = app().config();
        let asleep = app().windows.borrow().iter().find(|window| window.is_asleep()).cloned();
        let window = asleep.unwrap_or_else(|| {
            let window = Window::empty();
            window.add_scratch();
            window
        });
        let Some(scratch) = window.scratch_workspace() else {
            return window;
        };
        let folder =
            folder.unwrap_or_else(|| session::start_folder(&config, None, config.window.inherit_directory.window));
        let tab = window.make_tab(folder, program);
        scratch.tabs.borrow_mut().push(tab.clone());
        scratch.tabs.borrow_mut().select(TabId::new(tab.id));
        window.workspaces.borrow_mut().select(WorkspaceId::new(scratch.id));
        window.attach(&tab, scratch.id, None, None, Some(last::look(&config)));
        window.show_workspace(scratch.id, None);
        window.start_sessions(&tab);
        window.refresh();
        window
    }

    /// Adds the window's Terminal, in the folder the settings give new sessions; first, as the window has no other
    /// workspace yet.
    pub(super) fn add_scratch(&self) -> Rc<Workspace> {
        let base = session::start_folder(&app().config(), None, false);
        let workspace = Rc::new(Workspace::scratch(app().next_id(), base));
        self.workspaces.borrow_mut().push(workspace.clone());
        workspace
    }

    /// The window's Terminal.
    pub(crate) fn scratch_workspace(&self) -> Option<Rc<Workspace>> {
        self.workspaces().into_iter().find(|workspace| workspace.scratch)
    }

    /// Whether all its tabs were closed, so that no window of it shows, while it keeps its workspaces.
    pub(crate) fn is_asleep(&self) -> bool {
        !self.is_quick() && self.frames.borrow().is_empty() && !self.closing.get()
    }

    /// A window without workspaces or `NSWindow` yet, which the app knows.
    fn empty() -> Rc<Window> {
        let app = app();
        let window = Rc::new(Window {
            id: WindowId::new(app.next_id()),
            mtm: app.mtm,
            workspaces: RefCell::new(Ordered::new()),
            frames: RefCell::new(Vec::new()),
            quick: RefCell::new(None),
            closing: Cell::new(false),
            arranging: Cell::new(false),
            restoring: Cell::new(false),
            last_key: Cell::new(None),
            sidebar_setting: Cell::new(app.config().window.sidebar),
        });
        app.windows.borrow_mut().push(window.clone());
        window
    }

    /// The window's id.
    pub(crate) fn id(&self) -> u64 {
        self.id.get()
    }

    /// Whether `ns_window` is this window's: one of its tabs' windows or the quick terminal's panel.
    pub(crate) fn owns(&self, ns_window: &NSWindow) -> bool {
        let own = |window: &NSWindow| std::ptr::eq(window, ns_window);
        self.quick.borrow().as_ref().is_some_and(|quick| own(&quick.panel))
            || self.frames.borrow().iter().any(|frame| own(&frame.ns))
    }

    /// Whether the session view `view` is in one of the window's tabs.
    pub(crate) fn holds_view(&self, view: u64) -> bool {
        self.tab_of_view(view).is_some()
    }

    /// The `NSWindow` in front: that of the selected workspace's tab in front, or the quick terminal's panel.
    pub(crate) fn front_ns_window(&self) -> Option<Retained<NSWindow>> {
        if let Some(quick) = self.quick.borrow().as_ref() {
            return Some(quick.panel.clone());
        }
        Some(self.frame_of(self.selected_tab_id()?)?.ns.clone())
    }

    /// The window of tab `id`.
    fn frame_of(&self, id: u64) -> Option<Rc<TabWindow>> {
        self.frames.borrow().iter().find(|frame| frame.tab == id).cloned()
    }

    /// The window of the tab in front.
    fn front_frame(&self) -> Option<Rc<TabWindow>> {
        self.frame_of(self.selected_tab_id()?)
    }

    /// The id of the tab in front.
    pub(crate) fn selected_tab_id(&self) -> Option<u64> {
        self.selected_tab().map(|tab| tab.id)
    }

    /// The window's workspaces in their order.
    pub(crate) fn workspaces(&self) -> Vec<Rc<Workspace>> {
        self.workspaces.borrow().items().to_vec()
    }

    /// The workspace of the tab window in front.
    pub(crate) fn selected_workspace(&self) -> Option<Rc<Workspace>> {
        self.workspaces.borrow().selected()
    }

    /// The tabs of every workspace, workspace by workspace.
    pub(crate) fn tabs(&self) -> Vec<Rc<Tab>> {
        self.workspaces()
            .iter()
            .flat_map(|workspace| workspace.tabs())
            .collect()
    }

    /// The quick terminal's panel came to the front: what its tab needed attention for has had it.
    pub(crate) fn front_seen(&self) {
        if let Some(tab) = self.selected_tab() {
            tab.seen();
            self.refresh();
        }
    }

    fn selected_tab(&self) -> Option<Rc<Tab>> {
        self.selected_workspace()?.selected_tab()
    }

    fn tab_of_view(&self, view: u64) -> Option<Rc<Tab>> {
        self.workspaces()
            .iter()
            .find_map(|workspace| workspace.tab_of_view(view))
    }

    /// The workspace that holds tab `id`.
    fn workspace_of_tab(&self, id: u64) -> Option<Rc<Workspace>> {
        self.workspaces()
            .into_iter()
            .find(|workspace| workspace.tabs.borrow().get(TabId::new(id)).is_some())
    }

    /// The tab `id`, in whichever workspace.
    fn tab(&self, id: u64) -> Option<Rc<Tab>> {
        self.workspace_of_tab(id)?.tabs.borrow().get(TabId::new(id))
    }

    /// The session view of the active pane of the tab in front.
    pub(crate) fn focused_view(&self) -> Option<Retained<term_ui::TerminalView>> {
        self.selected_tab()?.focused_view()
    }

    /// Whether `view` is in the sidebar of the tab in front, with its panels.
    pub(crate) fn sidebar_holds(&self, view: &NSView) -> bool {
        self.front_frame()
            .is_some_and(|frame| view.isDescendantOf(&frame.sidebar_view))
    }

    fn make_tab(&self, folder: PathBuf, program: Option<Program>) -> Rc<Tab> {
        let (view, root) = session::make(self.mtm, self.id(), folder.clone(), program);
        Rc::new(Tab::new(app().next_id(), &view, root, folder))
    }

    /// Starts the shells of `tab` that have not started, once their views have their size in the tab's window.
    fn start_sessions(&self, tab: &Tab) {
        let own = self.frame_of(tab.id).map(|frame| frame.ns.clone());
        if let Some(window) = own.or_else(|| self.front_ns_window()) {
            window.layoutIfNeeded();
        }
        let sessions = tab.terminal().map(|terminal| terminal.panes.sessions());
        for view in sessions.unwrap_or_default().into_iter().filter_map(term_ui::find) {
            if view.with_session(|_| ()).is_none() {
                session::spawn(&view);
            }
        }
        if self.selected_tab_id() == Some(tab.id) {
            self.focus_active();
        }
    }

    /// Gives the keyboard to the active pane of the tab in front, in its window.
    fn focus_active(&self) {
        if let (Some(window), Some(editor)) = (self.front_ns_window(), self.front_editor()) {
            window.makeFirstResponder(Some(&editor));
            return;
        }
        let (Some(window), Some(view)) = (self.front_ns_window(), self.focused_view()) else {
            return;
        };
        window.makeFirstResponder(Some(&view));
        self.show_field_state();
    }

    /// The text view of the editor in the tab in front, if it is an editor tab.
    fn front_editor(&self) -> Option<Retained<objc2_app_kit::NSTextView>> {
        let workspace = self.selected_workspace()?;
        let tab = workspace.selected_tab()?;
        tab.as_hosted()
            .filter(|hosted| hosted.kind() == term_ui::workspace_host::TabKind::File)?;
        term_ui_editor::text_view_of_tab(&workspace.handle(), term_ui::workspace_host::TabId::new(tab.id))
    }

    /// Shows the input field's state of the active pane on the toolbar's switch.
    pub(crate) fn show_field_state(&self) {
        let Some(window) = self.front_ns_window() else {
            return;
        };
        toolbar::show_field(&window, self.focused_view().map(|view| view.field_look()));
    }

    /// What the toolbar's input field switch shows, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn field_switch(&self) -> String {
        self.front_ns_window()
            .map_or_else(|| "no window".into(), |window| toolbar::field_state(&window))
    }

    /// The toolbar's items and branch, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn toolbar_state(&self) -> String {
        self.front_ns_window()
            .map_or_else(|| "no window".into(), |window| toolbar::state(&window))
    }

    /// The entries of the menu of the `+` in the tab bar of the window in front, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn plus_menu_entries(&self) -> Vec<String> {
        self.selected_tab_id()
            .map_or_else(Vec::new, |tab| plus_menu::entries(self.id(), tab))
    }

    /// Chooses the entry titled `title` of the menu of the `+` in the tab bar of the window in front, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn choose_plus_entry(&self, title: &str) -> bool {
        self.selected_tab_id()
            .is_some_and(|tab| plus_menu::choose(self.id(), tab, title))
    }

    /// Follows the settings: the sidebars show or hide where their setting changed and rename on a click or not, and
    /// the tab titles take the interface size.
    pub(crate) fn config_changed(self: &Rc<Self>) {
        if self.is_quick() {
            return self.quick_config_changed();
        }
        let wanted = app().config().window.sidebar;
        if self.sidebar_setting.replace(wanted) != wanted && self.sidebar_shown() != wanted {
            self.flip_sidebar();
        }
        let click_renames = app().config().window.rename_on_click;
        for frame in self.frames.borrow().iter() {
            frame.invalidate_tab_title();
            frame.sidebar.set_click_renames(click_renames);
        }
        self.refresh();
    }

    /// Shows or hides the sidebar of the window in front and of the other tabs of its group, as its toolbar button
    /// does; new windows follow that choice, which the settings take over.
    pub(crate) fn toggle_sidebar(&self) {
        let Some(collapsed) = self.flip_sidebar() else {
            return;
        };
        let shown = !collapsed;
        for window in app().windows.borrow().iter() {
            window.sidebar_setting.set(shown);
        }
        if app().config().window.sidebar != shown {
            let written = crate::settings::apply(|doc| {
                doc.set(&["window", "sidebar"], shown);
                Ok(true)
            });
            if let Err(error) = written {
                term_base::diag::report("the sidebar setting", error);
            }
        }
    }

    /// Shows or hides the sidebar of the window in front and of the other tabs of its group; whether it is now hidden.
    fn flip_sidebar(&self) -> Option<bool> {
        let front = self.front_frame()?;
        let collapsed = !front.look().collapsed;
        front.slide_sidebar(collapsed);
        let width = front.look().width;
        for frame in self.frames.borrow().iter() {
            if frame.tab != front.tab && front.groups_with(&frame.ns) {
                frame.apply(Look { collapsed, width });
            }
        }
        last::remember(&front, Look { collapsed, width });
        Some(collapsed)
    }

    /// Whether the window in front shows its sidebar.
    pub(crate) fn sidebar_shown(&self) -> bool {
        self.front_frame().is_some_and(|frame| !frame.look().collapsed)
    }

    // MARK: reports

    /// Takes up what session `view` reported.
    pub(crate) fn session_changed(self: &Rc<Self>, view: u64, change: Change) {
        let Some(tab) = self.tab_of_view(view) else { return };
        if change == Change::Field {
            if self.focused_view().is_some_and(|front| front.id() == view) {
                self.show_field_state();
            }
            return;
        }
        if change == Change::Focused {
            if let Some(terminal) = tab.terminal() {
                terminal.panes.focus(view);
            }
            self.refresh();
            return;
        }
        let shell_done = matches!(change, Change::Exited(term_base::Ending::Code(0)))
            && term_ui::find(view).is_some_and(|view| !view.runs_command());
        if shell_done {
            self.close_session(&tab, view);
            return;
        }
        if matches!(change, Change::Prompt | Change::Directory(_))
            && let Some(workspace) = self.workspace_of_tab(tab.id)
        {
            workspace.refresh_branch();
            // A command may have changed the repository, which no watcher sees while the file list is closed.
            term_ui_git::changed(&workspace.base);
        }
        let in_front =
            self.selected_tab_id() == Some(tab.id) && self.front_ns_window().is_some_and(|window| window.isKeyWindow());
        if tab.changed(view, &change, in_front) {
            self.refresh();
        }
    }

    /// Shows the workspaces' rows in every sidebar on screen, each tab's title on its window and tab, and the toolbars
    /// as they are now.
    fn refresh(&self) {
        if let Some(quick) = self.quick.borrow().as_ref() {
            if let Some(tab) = self.selected_tab() {
                quick.panel.setTitle(&term_ui::kit::ns(&tab.title()));
            }
            return;
        }
        let rows: Vec<term_ui_sidebar::Workspace> =
            self.workspaces().iter().map(|workspace| workspace.entry()).collect();
        // Entries already queried the foreground process of each pane. Reuse them for window and toolbar titles.
        let entries: HashMap<_, _> = rows
            .iter()
            .flat_map(|row| row.tabs.iter().map(move |entry| (entry.id, (row, entry))))
            .collect();
        let frames = self.frames.borrow().clone();
        let selected = self.selected_tab_id();
        let home = term_base::home();
        for frame in frames {
            let Some(&(row, entry)) = entries.get(&frame.tab) else {
                continue;
            };
            let Some(workspace) = self.workspaces.borrow().get(WorkspaceId::new(frame.workspace)) else {
                continue;
            };
            let Some(tab) = workspace.tabs.borrow().get(TabId::new(frame.tab)) else {
                continue;
            };
            frame.update_tab_title(&tab, entry, &home);
            frame.show_mark(entry.mark());
            let heading = term_ui_sidebar::model::tilde(&entry.folder.to_string_lossy(), &home);
            let branch = entry
                .branch
                .as_deref()
                .or_else(|| (entry.folder == row.base).then_some(row.branch.as_deref()).flatten())
                .unwrap_or_default();
            title::show(&frame.ns, &heading, branch);
            if !term_ui::kit::window_in_front(&frame.ns) && selected != Some(frame.tab) {
                continue;
            }
            frame.sidebar.show(rows.clone(), Some(workspace.id));
            toolbar::show_field(&frame.ns, tab.focused_view().map(|view| view.field_look()));
            // AppKit makes the toolbar's items only once the window shows, after the sidebar's look was applied.
            toolbar::follow_sidebar(&frame.ns, frame.sidebar_collapsed());
        }
        self.show_panels();
    }
}

/// Puts `view` into `body`, filling it below the toolbar and the tab bar.
pub(super) fn fill(body: &NSView, view: &NSView) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    body.addSubview(view);
    let guide = body.safeAreaLayoutGuide();
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        view.leadingAnchor().constraintEqualToAnchor(&body.leadingAnchor()),
        view.trailingAnchor().constraintEqualToAnchor(&body.trailingAnchor()),
        view.bottomAnchor().constraintEqualToAnchor(&body.bottomAnchor()),
        view.topAnchor().constraintEqualToAnchor(&guide.topAnchor()),
    ]));
}

/// The app came to the front: the editors look at their files, which may have changed while no watcher ran, and the
/// Git panels read their repositories again.
pub(crate) fn app_activated() {
    term_ui_editor::check_disk();
    let Some(app) = crate::state::try_app() else { return };
    for window in app.windows.borrow().iter() {
        for workspace in window.workspaces() {
            term_ui_git::changed(&workspace.base);
        }
    }
}
