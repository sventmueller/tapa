//! The window of one tab (E20, F39): a native window in its workspace's tab group, with its own toolbar and a
//! full-height sidebar of all the window's workspaces, as each Finder tab has its own sidebar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAnimatablePropertyContainer, NSBackingStoreType, NSImageView, NSSplitViewController,
    NSSplitViewItem, NSView, NSViewController, NSWindow, NSWindowButton, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use term_base::guarded;
use term_ui::kit::ns;
use term_ui_sidebar::{Mark, Sidebar};

use super::delegate::Delegate;
use super::toolbar::Toolbar;
use super::{Window, geometry};
use crate::state::{app, try_app};

/// Which tab of which window an `NSWindow` shows.
pub(crate) struct Ivars {
    window: u64,
    tab: u64,
    /// Whether AppKit asked for its key view loop while a tab group was being built, which waits until it is key.
    stale_loop: Cell<bool>,
}

thread_local! {
    /// Set while a tab group is built out of sight.
    static BUILDING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `build`, in which windows join a tab group, with the key view loops of the windows put off until each is
/// key: AppKit works each out as a window joins, which lays out the window's whole view tree, also for tabs out of
/// sight.
pub(super) fn building<T>(build: impl FnOnce() -> T) -> T {
    BUILDING.with(|building| {
        let _building = term_ui::kit::FlagGuard::raise(building);
        build()
    })
}

define_class!(
    // SAFETY: NSWindow has no subclassing requirements and `TabNsWindow` does not implement Drop.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermTabWindow"]
    #[ivars = Ivars]
    pub(crate) struct TabNsWindow;

    impl TabNsWindow {
        /// The tab bar's `+`: the menu of what a new tab beside this one can show (E20).
        #[unsafe(method(newWindowForTab:))]
        fn new_window_for_tab(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                let Ivars { window, tab, .. } = *self.ivars();
                super::plus_menu::open(self, window, tab);
            });
        }

        /// The close button: the window closes as Close Window does, where AppKit would close the tab's window alone.
        #[unsafe(method(closeGroup:))]
        fn close_group(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window, _| window.close_window()));
        }

        #[unsafe(method(recalculateKeyViewLoop))]
        fn recalculate_key_view_loop(&self) {
            if BUILDING.get() {
                self.ivars().stale_loop.set(true);
                return;
            }
            // SAFETY: The overridden method, which takes and returns nothing.
            let () = unsafe { msg_send![super(self), recalculateKeyViewLoop] };
        }

        #[unsafe(method(becomeKeyWindow))]
        fn become_key_window(&self) {
            // SAFETY: The overridden method, which takes and returns nothing.
            let () = unsafe { msg_send![super(self), becomeKeyWindow] };
            if self.ivars().stale_loop.replace(false) {
                // SAFETY: As above.
                let () = unsafe { msg_send![super(self), recalculateKeyViewLoop] };
            }
        }

        /// Window > Merge All Windows: the windows of this tab's workspace, as only they share its tab group.
        #[unsafe(method(mergeAllWindows:))]
        fn merge_all_windows(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window, tab| window.merge_all(tab)));
        }
    }
);

impl TabNsWindow {
    fn on_window(&self, act: impl FnOnce(&Rc<Window>, u64)) {
        let Ivars { window, tab, .. } = *self.ivars();
        if let Some(window) = try_app().and_then(|app| app.window(window)) {
            act(&window, tab);
        }
    }
}

/// How a window shows its sidebar, which the next tab of its group takes over.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Look {
    pub(crate) collapsed: bool,
    pub(crate) width: f64,
}

/// The window of one tab with what it is made of.
pub(crate) struct TabWindow {
    pub(crate) tab: u64,
    pub(crate) workspace: u64,
    pub(crate) ns: Retained<NSWindow>,
    _delegate: Retained<Delegate>,
    _toolbar: Retained<Toolbar>,
    pub(crate) sidebar: Retained<Sidebar>,
    pub(crate) sidebar_view: Retained<NSView>,
    sidebar_item: Retained<NSSplitViewItem>,
    split: Retained<NSSplitViewController>,
    /// The sidebar's width while it showed last.
    width: Cell<f64>,
    /// The watch on the sidebar's width, which the other windows of the tab group take over at once.
    _divider: term_ui::kit::Observation,
    /// The dot at the trailing end of the window's tab while its agent works or waits, as in Dango.
    status: Retained<NSImageView>,
    tab_title: RefCell<Option<String>>,
    tab_mark: Cell<Option<Mark>>,
    /// The watches that keep the sidebar below the toolbar in full screen.
    _full_screen: Vec<term_ui::kit::Observation>,
}

impl TabWindow {
    /// The window of tab `tab` of workspace `workspace` in `window`, showing `content`, at `frame` or where a new
    /// window opens; not on screen yet.
    pub(crate) fn new(
        window: &Window,
        (tab, workspace): (u64, u64),
        content: &NSView,
        frame: Option<NSRect>,
        look: Look,
    ) -> Rc<TabWindow> {
        let mtm = window.mtm;
        let ns_window = make_ns_window(mtm, window.id(), tab);
        // SAFETY: The app keeps its own reference, so closing must not release the window under it.
        unsafe { ns_window.setReleasedWhenClosed(false) };
        ns_window.setTabbingIdentifier(&ns(&format!("term.workspace.{workspace}")));
        let toolbar = Toolbar::attach(mtm, &ns_window, window.id(), (tab, workspace));
        let delegate = Delegate::new(mtm, window.id(), Some(tab));
        ns_window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let (sidebar, sidebar_view) = make_sidebar(mtm, window.id(), tab);
        let sidebar_controller = NSViewController::new(mtm);
        sidebar_controller.setView(&sidebar_view);
        let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_controller);
        sidebar_item.setMinimumThickness(geometry::SIDEBAR_MIN);
        sidebar_item.setMaximumThickness(geometry::SIDEBAR_MAX);
        // Above the content's, so resizing the window resizes the content and the sidebar keeps its width.
        sidebar_item.setHoldingPriority(260.0);
        sidebar_item.setCanCollapse(true);
        let body = NSView::new(mtm);
        super::fill(&body, content);
        let content_controller = NSViewController::new(mtm);
        content_controller.setView(&body);
        let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_controller);
        content_item.setMinimumThickness(320.0);
        let split = NSSplitViewController::new(mtm);
        split.addSplitViewItem(&sidebar_item);
        split.addSplitViewItem(&content_item);
        let initial = ns_window.frame();
        ns_window.setContentViewController(Some(&split));
        ns_window.setContentMinSize(geometry::min_content(true));
        match frame {
            Some(frame) => ns_window.setFrame_display(frame, false),
            None => place_new(&ns_window, initial),
        }
        let divider = watch_divider(mtm, &split, window.id(), tab);
        let full_screen = super::full_screen::follow(&ns_window, &sidebar_view, &body);
        let this = Rc::new(TabWindow {
            tab,
            workspace,
            ns: ns_window,
            _delegate: delegate,
            _toolbar: toolbar,
            sidebar,
            sidebar_view,
            sidebar_item,
            split,
            width: Cell::new(look.width),
            _divider: divider,
            status: status_dot(mtm),
            tab_title: RefCell::new(None),
            tab_mark: Cell::new(None),
            _full_screen: full_screen,
        });
        this.apply(look);
        this
    }

    /// Rebuilds the attributed title only when its content or icon changed.
    pub(super) fn update_tab_title(
        &self,
        tab: &crate::tab::Tab,
        entry: &term_ui_sidebar::model::Entry,
        home: &std::path::Path,
    ) {
        let title = entry.heading(home);
        if self.tab_title.borrow().as_ref() != Some(&title) {
            *self.tab_title.borrow_mut() = Some(title.clone());
            let text = ns(&title);
            self.ns.setTitle(&text);
            self.ns.tab().setTitle(Some(&text));
            self.ns
                .tab()
                .setAttributedTitle(Some(&super::tabs::tab_title(tab, &title)));
        }
        let subtitle = ns(&tab.subtitle(entry, home));
        if self.ns.subtitle() != subtitle {
            self.ns.setSubtitle(&subtitle);
        }
    }

    pub(super) fn invalidate_tab_title(&self) {
        self.tab_title.borrow_mut().take();
    }

    /// Shows `mark`'s dot on the window's tab, or none.
    pub(crate) fn show_mark(&self, mark: Option<Mark>) {
        if self.tab_mark.replace(mark) == mark {
            return;
        }
        let tab = self.ns.tab();
        if let Some(mark) = mark {
            self.status
                .setContentTintColor(Some(&term_ui_sidebar::mark_color(mark)));
            self.status.setAccessibilityLabel(Some(&ns(mark.spoken())));
        }
        let current = tab.accessoryView(self.ns.mtm());
        if mark.is_some() && current.is_none() {
            tab.setAccessoryView(Some(&self.status));
        } else if mark.is_none() && current.is_some() {
            tab.setAccessoryView(None);
        }
    }

    /// Has macOS end the toolbar and the tab bar over the content with the soft edge it gives scrolling content, not
    /// with the line it draws over the rest; for a terminal, whose text starts right below.
    pub(crate) fn soften_edge(&self) {
        let mtm = self.ns.mtm();
        let Some(body) = self
            .split
            .splitViewItems()
            .iter()
            .nth(1)
            .map(|item| item.viewController(mtm).view())
        else {
            return;
        };
        // macOS 26 picks the edge by whether a scroll view lies under the bars; this one is empty and draws nothing.
        let scroll = objc2_app_kit::NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(false);
        scroll.setHasHorizontalScroller(false);
        scroll.setDocumentView(Some(&NSView::new(mtm)));
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        body.addSubview_positioned_relativeTo(&scroll, objc2_app_kit::NSWindowOrderingMode::Below, None);
        term_ui::kit::pin_edges(&scroll, &body);
    }

    /// How the sidebar shows now.
    pub(crate) fn look(&self) -> Look {
        let collapsed = self.sidebar_item.isCollapsed();
        if !collapsed {
            let width = self.sidebar_view.frame().size.width;
            if width >= geometry::SIDEBAR_MIN {
                self.width.set(width);
            }
        }
        Look {
            collapsed,
            width: self.width.get(),
        }
    }

    /// Whether the sidebar is collapsed.
    pub(crate) fn sidebar_collapsed(&self) -> bool {
        self.sidebar_item.isCollapsed()
    }

    /// Shows the sidebar as `look` says, at once.
    pub(crate) fn apply(&self, look: Look) {
        self.width.set(look.width);
        if self.sidebar_item.isCollapsed() != look.collapsed {
            self.sidebar_item.setCollapsed(look.collapsed);
        }
        super::toolbar::follow_sidebar(&self.ns, look.collapsed);
        if !look.collapsed && (self.sidebar_view.frame().size.width - look.width).abs() >= 1.0 {
            self.split.splitView().setPosition_ofDividerAtIndex(look.width, 0);
        }
    }

    /// Shows or hides the sidebar, sliding, as its toolbar button does.
    pub(crate) fn slide_sidebar(&self, collapsed: bool) {
        self.look();
        super::toolbar::follow_sidebar(&self.ns, collapsed);
        self.sidebar_item.animator().setCollapsed(collapsed);
    }

    /// The windows of its tab group in the order of the tab bar; itself alone outside one.
    pub(crate) fn group(&self) -> Vec<Retained<NSWindow>> {
        let windows = self.ns.tabbedWindows().map(|windows| windows.to_vec());
        windows
            .filter(|windows| windows.iter().any(|window| std::ptr::eq(&**window, &*self.ns)))
            .unwrap_or_else(|| vec![self.ns.clone()])
    }

    /// Whether it is on screen, in front or as a tab of a group that is.
    pub(crate) fn is_shown(&self) -> bool {
        self.group().iter().any(|window| window.isVisible())
    }

    /// Whether `other` is in its tab group.
    pub(crate) fn groups_with(&self, other: &NSWindow) -> bool {
        self.group().iter().any(|window| std::ptr::eq(&**window, other))
    }

    /// Takes it off screen and out of its tab group, kept for when its workspace shows again.
    pub(crate) fn hide(&self) {
        if let Some(group) = self.ns.tabGroup()
            && group.windows().len() > 1
        {
            group.removeWindow(&self.ns);
        }
        // Without this the window fades out over the one that takes its place.
        let animation = self.ns.animationBehavior();
        self.ns
            .setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::None);
        self.ns.orderOut(None);
        self.ns.setAnimationBehavior(animation);
        self.ns.setExcludedFromWindowsMenu(true);
    }
}

fn status_dot(mtm: MainThreadMarker) -> Retained<NSImageView> {
    let status = NSImageView::new(mtm);
    status.setImage(term_ui::kit::symbol_at("circle.fill", "", 7.0).as_deref());
    status
}

fn make_ns_window(mtm: MainThreadMarker, window: u64, tab: u64) -> Retained<NSWindow> {
    // The sidebar runs up under the traffic lights, as in Finder and Mail.
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let config = app().config();
    let frame = geometry::content_rect(&config, true);
    let this = TabNsWindow::alloc(mtm).set_ivars(Ivars {
        window,
        tab,
        stale_loop: Cell::new(false),
    });
    // SAFETY: NSWindow's designated initializer, called once on the newly allocated window of the main thread.
    let this: Retained<TabNsWindow> = unsafe {
        msg_send![
            super(this),
            initWithContentRect: frame,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };
    if let Some(button) = this.standardWindowButton(NSWindowButton::CloseButton) {
        // SAFETY: The button's target is the window that owns it, so it outlives the button's weak reference.
        unsafe {
            button.setTarget(Some(&this));
            button.setAction(Some(objc2::sel!(closeGroup:)));
        }
    }
    Retained::into_super(this)
}

/// Has the other windows of tab `tab`'s group take over its sidebar as the user drags its divider or collapses it, so
/// that the next tab shows at once as this one does rather than changing as it comes to the front.
fn watch_divider(
    mtm: MainThreadMarker,
    split: &NSSplitViewController,
    window: u64,
    tab: u64,
) -> term_ui::kit::Observation {
    // SAFETY: AppKit sets its exported notification names when it loads and never changes them.
    let name = unsafe { objc2_app_kit::NSSplitViewDidResizeSubviewsNotification };
    let split_view = split.splitView();
    let object: &AnyObject = &split_view;
    term_ui::kit::observe(mtm, name, Some(object), move |_| {
        let Some(window) = try_app().and_then(|app| app.window(window)) else {
            return;
        };
        if window.frame_of(tab).is_some_and(|frame| frame.ns.isKeyWindow()) {
            window.sync_look(tab);
        }
    })
}

/// Gives a window that opens on its own the size of the window last resized, else its size from the settings: beside
/// the window in front, else where the last one was while that is on a screen, else centred.
fn place_new(ns_window: &NSWindow, initial: NSRect) {
    let config = app().config();
    let last = super::last::frame(&config);
    match last {
        Some(last) => ns_window.setFrame_display(NSRect::new(initial.origin, last.size), false),
        None => {
            ns_window.setFrame_display(initial, false);
            geometry::fit_toolbar(ns_window, geometry::content_rect(&config, true).size);
        }
    }
    if let Some(front) = app().front().and_then(|front| front.front_ns_window()) {
        let at = front.frame();
        ns_window.setFrameTopLeftPoint(NSPoint::new(at.origin.x + 24.0, at.origin.y + at.size.height - 24.0));
    } else if let Some(last) = last.filter(|last| geometry::on_a_screen(objc2::MainThreadOnly::mtm(ns_window), *last)) {
        ns_window.setFrameOrigin(last.origin);
    } else {
        ns_window.center();
    }
    geometry::fit_screen(ns_window);
}

/// The sidebar of tab `tab`'s window, which swaps the workspace its window shows and runs its rows' menus.
fn make_sidebar(mtm: MainThreadMarker, window: u64, tab: u64) -> (Retained<Sidebar>, Retained<NSView>) {
    let (sidebar, view) = Sidebar::new(mtm, move |workspace| {
        // Later, as the window of this sidebar leaves the screen with the workspace it showed.
        term_base::main_soon(move || {
            if let Some(window) = try_app().and_then(|app| app.window(window)) {
                window.pick_workspace(tab, workspace);
            }
        });
    });
    sidebar.on_action(move |workspace, action| {
        // Later, as the sidebar's menu is still busy with the click.
        term_base::main_soon(move || {
            if let Some(window) = try_app().and_then(|app| app.window(window)) {
                window.row_action(workspace, action);
            }
        });
    });
    sidebar.on_rename(move |workspace, name| {
        // Later, as the field still ends its editing.
        term_base::main_soon(move || {
            if let Some(window) = try_app().and_then(|app| app.window(window)) {
                window.set_workspace_name(workspace, &name);
            }
        });
    });
    sidebar.on_move(move |workspace, index| {
        // Later, as the table still ends the drag.
        term_base::main_soon(move || {
            if let Some(window) = try_app().and_then(|app| app.window(window)) {
                window.move_workspace(workspace, index);
            }
        });
    });
    sidebar.on_settings(|| term_base::main_soon(crate::settings::show));
    sidebar.on_panel_toggle(move |workspace, panel| {
        // Later, as the table reloads the row whose switch was clicked.
        term_base::main_soon(move || {
            if let Some(window) = try_app().and_then(|app| app.window(window)) {
                window.toggle_panel(workspace, panel);
            }
        });
    });
    (sidebar, view)
}
