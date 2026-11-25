//! A window's toolbar in the unified style (E20, E22). Over the sidebar: the system's sidebar button and New Workspace,
//! whose menu offers the folders used last and a folder dialog. Over the content, from the trailing edge: one group of
//! a button per kind of content a tab holds (terminals, agents with their menu, browsers); left of it a group of what
//! acts on a terminal tab's panes, greyed in other tabs: the two splits as in Dango and the input field's switch,
//! which acts on the pane in front (E7, F209) and shows in the accent colour while the field is on; left of that the branch
//! pull-down of the Git panel with "Neuer Worktree …".

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSMenu, NSMenuDelegate, NSMenuItem, NSMenuToolbarItem, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemValidation,
    NSToolbarSidebarTrackingSeparatorItemIdentifier, NSToolbarSpaceItemIdentifier,
    NSToolbarToggleSidebarItemIdentifier, NSWindow, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};
use splat_i18n::t;
use term_base::guarded;
use term_ui::FieldLook;
use term_ui::kit::{self, ns};
use term_ui_panes::Side;

use crate::state::try_app;

const NEW_WORKSPACE: &str = "term.new-workspace";
const BRANCH: &str = term_ui_git::TOOLBAR_ID;
const NEW_TERMINAL: &str = "term.new-terminal";
const NEW_AGENT: &str = "term.new-agent";
const NEW_BROWSER: &str = "term.new-browser";
const SPLIT_RIGHT: &str = "term.split-right";
const SPLIT_DOWN: &str = "term.split-down";
const FIELD: &str = "term.field";
/// The input field's switch: a box with a character, as an input field, rather than a keyboard (E22).
const FIELD_SYMBOL: &str = "character.textbox";
/// The point size of the switch's symbol while the field is on.
const ON_POINTS: f64 = 17.0;

/// Which window, workspace and tab the toolbar belongs to, its menu and the folders its items stand for.
pub(crate) struct Ivars {
    window: u64,
    workspace: u64,
    tab: u64,
    new_menu: RefCell<Option<Retained<NSMenu>>>,
    /// The folders of the New Workspace menu's items, by their tag.
    recent: RefCell<Vec<PathBuf>>,
    agent_menu: RefCell<Option<Retained<crate::agent::AgentMenu>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Toolbar` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermToolbarDelegate"]
    #[ivars = Ivars]
    pub(crate) struct Toolbar;

    unsafe impl NSObjectProtocol for Toolbar {}

    unsafe impl NSToolbarDelegate for Toolbar {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, identifier: &NSString, _flag: bool) -> Option<Retained<NSToolbarItem>> {
            guarded(None, || self.make_item(identifier))
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            self.identifiers()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            self.identifiers()
        }

    }

    unsafe impl NSToolbarItemValidation for Toolbar {
        #[unsafe(method(validateToolbarItem:))]
        fn validate_toolbar_item(&self, item: &NSToolbarItem) -> bool {
            guarded(true, || self.validate(item))
        }
    }

    unsafe impl NSMenuDelegate for Toolbar {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            guarded((), || self.fill(menu));
        }
    }

    impl Toolbar {
        #[unsafe(method(termNewWorkspace:))]
        fn new_workspace(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| window.new_workspace()));
        }

        #[unsafe(method(termOpenRecent:))]
        fn open_recent(&self, item: &NSMenuItem) {
            guarded((), || {
                let folder = usize::try_from(item.tag())
                    .ok()
                    .and_then(|at| self.ivars().recent.borrow().get(at).cloned());
                if let Some(folder) = folder {
                    self.on_window(move |window| window.open_workspace(folder));
                }
            });
        }

        #[unsafe(method(termNewTab:))]
        fn new_tab(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| window.new_tab()));
        }

        #[unsafe(method(termNewBrowser:))]
        fn new_browser(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| crate::browser::open(window, None)));
        }

        #[unsafe(method(termToggleSidebar:))]
        fn toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| window.toggle_sidebar()));
        }

        #[unsafe(method(termSplitRight:))]
        fn split_right(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| window.split(Side::Right)));
        }

        #[unsafe(method(termSplitDown:))]
        fn split_down(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.on_window(|window| window.split(Side::Below)));
        }

        #[unsafe(method(termToggleField:))]
        fn toggle_field(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                self.on_window(|window| {
                    if let Some(view) = window.focused_view() {
                        super::toggle_field(&view);
                    }
                    window.show_field_state();
                });
            });
        }
    }
);

impl Toolbar {
    /// Gives `ns_window`, of window `window`, its toolbar; the returned delegate must live as long as the toolbar.
    pub(crate) fn attach(
        mtm: MainThreadMarker,
        ns_window: &NSWindow,
        window: u64,
        (tab, workspace): (u64, u64),
    ) -> Retained<Toolbar> {
        let this = Toolbar::alloc(mtm).set_ivars(Ivars {
            window,
            workspace,
            tab,
            new_menu: RefCell::new(None),
            recent: RefCell::new(Vec::new()),
            agent_menu: RefCell::new(None),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        let delegate: Retained<Toolbar> = unsafe { msg_send![super(this), init] };
        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("term-workspaces"));
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.setAllowsUserCustomization(false);
        toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        // The title shows as the toolbar's first item over the content, where it lines up with the tabs.
        ns_window.setTitleVisibility(objc2_app_kit::NSWindowTitleVisibility::Hidden);
        ns_window.setToolbar(Some(&toolbar));
        delegate.take_sidebar_button(&toolbar);
        delegate
    }

    /// Has the system's sidebar button show or hide the sidebars of the whole tab group, not of this window alone.
    fn take_sidebar_button(&self, toolbar: &NSToolbar) {
        // SAFETY: AppKit's constant identifier, which lives as long as the process.
        let sidebar = unsafe { NSToolbarToggleSidebarItemIdentifier };
        let target: &AnyObject = self;
        for item in toolbar.items().iter() {
            if *item.itemIdentifier() == *sidebar {
                // SAFETY: The toolbar's delegate answers this action and outlives the toolbar's items.
                unsafe {
                    item.setTarget(Some(target));
                    item.setAction(Some(sel!(termToggleSidebar:)));
                }
            }
        }
    }

    /// Whether `item` can act now: the splits only in a terminal tab, whose panes they split.
    fn validate(&self, item: &NSToolbarItem) -> bool {
        let id = item.itemIdentifier().to_string();
        if id != SPLIT_RIGHT && id != SPLIT_DOWN {
            return true;
        }
        let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) else {
            return false;
        };
        window.tab(self.ivars().tab).is_some_and(|tab| tab.terminal().is_some())
    }

    /// Runs `act` on the toolbar's window.
    fn on_window(&self, act: impl FnOnce(&std::rc::Rc<super::Window>)) {
        if let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) {
            act(&window);
        }
    }

    fn identifiers(&self) -> Retained<NSArray<NSString>> {
        // SAFETY: AppKit's constant identifiers, which live as long as the process.
        let (sidebar, tracking, flexible, space) = unsafe {
            (
                NSToolbarToggleSidebarItemIdentifier,
                NSToolbarSidebarTrackingSeparatorItemIdentifier,
                NSToolbarFlexibleSpaceItemIdentifier,
                NSToolbarSpaceItemIdentifier,
            )
        };
        let own: Vec<Retained<NSString>> = [
            NEW_WORKSPACE,
            super::title::ID,
            BRANCH,
            NEW_TERMINAL,
            NEW_AGENT,
            NEW_BROWSER,
            SPLIT_RIGHT,
            SPLIT_DOWN,
            FIELD,
        ]
        .into_iter()
        .map(ns)
        .collect();
        let [
            new_workspace,
            title,
            branch,
            terminal,
            agent,
            browser,
            right,
            down,
            field,
        ] = own.as_slice()
        else {
            return NSArray::new();
        };
        // A space between two groups of buttons keeps them apart, each in a capsule of its own.
        let ids: Vec<&NSString> = vec![
            sidebar,
            flexible,
            new_workspace,
            tracking,
            title,
            flexible,
            branch,
            space,
            right,
            down,
            field,
            space,
            terminal,
            agent,
            browser,
        ];
        NSArray::from_slice(&ids)
    }

    fn make_item(&self, identifier: &NSString) -> Option<Retained<NSToolbarItem>> {
        let mtm = self.mtm();
        let target: &AnyObject = self;
        let name = identifier.to_string();
        let button = |title: &str, symbol: &str, action: Option<Sel>| {
            let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), identifier);
            let label = ns(title);
            item.setImage(kit::symbol(symbol, title).as_deref());
            item.setLabel(&label);
            item.setPaletteLabel(&label);
            item.setToolTip(Some(&label));
            item.setBordered(true);
            match action {
                // SAFETY: The toolbar's delegate answers the actions given here and outlives the toolbar's items.
                Some(action) => unsafe {
                    item.setTarget(Some(target));
                    item.setAction(Some(action));
                },
                None => {
                    item.setAutovalidates(false);
                    item.setEnabled(false);
                }
            }
            item
        };
        let item = match name.as_str() {
            NEW_WORKSPACE => return Some(self.new_workspace_item(identifier)),
            super::title::ID => return Some(super::title::item(mtm, identifier)),
            BRANCH => {
                let workspace = term_ui::workspace_host::WorkspaceId::new(self.ivars().workspace);
                return Some(term_ui_git::toolbar_item(&term_ui::workspace_host::Workspace::new(
                    workspace,
                )));
            }
            NEW_TERMINAL => button(t("Neue Terminal-Sitzung"), "apple.terminal", Some(sel!(termNewTab:))),
            NEW_AGENT => {
                let (item, menu) = crate::agent::AgentMenu::item(mtm, identifier, self.ivars().window);
                *self.ivars().agent_menu.borrow_mut() = Some(menu);
                item
            }
            NEW_BROWSER => button(t("Neue Browser-Sitzung"), "globe", Some(sel!(termNewBrowser:))),
            SPLIT_RIGHT => button(t("Rechts teilen"), "rectangle.split.2x1", Some(sel!(termSplitRight:))),
            SPLIT_DOWN => button(t("Unten teilen"), "rectangle.split.1x2", Some(sel!(termSplitDown:))),
            FIELD => {
                let item = button(FieldLook::name(), FIELD_SYMBOL, Some(sel!(termToggleField:)));
                // The switch's state is the pane's, which validation by target and action would not know.
                item.setAutovalidates(false);
                item
            }
            _ => return None,
        };
        Some(item)
    }

    /// New Workspace: a click shows the folders used last and the folder dialog right below it.
    fn new_workspace_item(&self, identifier: &NSString) -> Retained<NSToolbarItem> {
        let mtm = self.mtm();
        let item = NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), identifier);
        let label = ns(t("Neuer Arbeitsbereich"));
        item.setImage(kit::symbol("folder.badge.plus", t("Neuer Arbeitsbereich")).as_deref());
        item.setLabel(&label);
        item.setPaletteLabel(&label);
        item.setToolTip(Some(&label));
        item.setShowsIndicator(false);
        item.setAutovalidates(false);
        item.setEnabled(true);
        let menu = NSMenu::new(mtm);
        // AppKit never opens an empty menu, so it would never ask the delegate to fill it.
        self.fill_recent(&menu);
        menu.setDelegate(Some(ProtocolObject::from_ref(self)));
        item.setMenu(&menu);
        *self.ivars().new_menu.borrow_mut() = Some(menu);
        Retained::into_super(item)
    }

    /// Fills the menu that is about to open: the recent folders.
    fn fill(&self, menu: &NSMenu) {
        let is_new = self
            .ivars()
            .new_menu
            .borrow()
            .as_deref()
            .is_some_and(|own| std::ptr::eq(own, menu));
        if is_new {
            self.fill_recent(menu);
        }
    }

    fn fill_recent(&self, menu: &NSMenu) {
        let folders = crate::recent::folders();
        // AppKit asks again while an item is being chosen; rebuilding then would drop the chosen item's action.
        if menu.numberOfItems() > 0 && *self.ivars().recent.borrow() == folders {
            return;
        }
        menu.removeAllItems();
        let mtm = self.mtm();
        let home = term_base::home();
        for (at, folder) in folders.iter().enumerate() {
            let title = term_ui_sidebar::model::tilde(&folder.to_string_lossy(), &home);
            let item = kit::tagged(mtm, &title, sel!(termOpenRecent:), "", isize::try_from(at).unwrap_or(0));
            item.setImage(kit::symbol("folder", "").as_deref());
            kit::set_target(&item, self as &AnyObject);
            menu.addItem(&item);
        }
        if !folders.is_empty() {
            menu.addItem(&kit::separator(mtm));
        }
        let choose = kit::item(mtm, t("Ordner wählen …"), Some(sel!(termNewWorkspace:)), "");
        kit::set_target(&choose, self as &AnyObject);
        menu.addItem(&choose);
        *self.ivars().recent.borrow_mut() = folders;
    }
}

/// Hides New Workspace, which belongs to the sidebar, while the sidebar is `collapsed`; else macOS moves it into an
/// overflow menu of the toolbar, a second way to what the File menu offers.
pub(crate) fn follow_sidebar(ns_window: &NSWindow, collapsed: bool) {
    let Some(toolbar) = ns_window.toolbar() else { return };
    let id = ns(NEW_WORKSPACE);
    if let Some(item) = toolbar.items().iter().find(|item| *item.itemIdentifier() == *id)
        && item.isHidden() != collapsed
    {
        item.setHidden(collapsed);
    }
}

/// The toolbar delegate of `ns_window`, one of the app's.
#[cfg(feature = "selftest")]
fn delegate_of(ns_window: &NSWindow) -> Option<Retained<Toolbar>> {
    let delegate = ns_window.toolbar()?.delegate()?;
    let object: Retained<AnyObject> = delegate.into();
    object.downcast::<Toolbar>().ok()
}

/// Shows `look`, the input field's state of the terminal pane in front, on the switch in `ns_window`'s toolbar;
/// without a terminal in front the switch is greyed.
pub(crate) fn show_field(ns_window: &NSWindow, look: Option<FieldLook>) {
    let Some(toolbar) = ns_window.toolbar() else { return };
    let field = ns(FIELD);
    let Some(item) = toolbar.items().iter().find(|item| *item.itemIdentifier() == *field) else {
        return;
    };
    let on = look == Some(FieldLook::On);
    item.setEnabled(on || look == Some(FieldLook::Off));
    // A selectable item would take a capsule of its own, apart from the splits it belongs with.
    if item.image().is_none_or(|image| image.isTemplate() == on) {
        let image = if on {
            // A tinted image is drawn as it is, so it needs the size the toolbar gives its symbols.
            kit::symbol_at(FIELD_SYMBOL, FieldLook::name(), ON_POINTS)
                .map(|symbol| kit::tinted(&symbol, &objc2_app_kit::NSColor::controlAccentColor()))
        } else {
            kit::symbol(FIELD_SYMBOL, FieldLook::name())
        };
        item.setImage(image.as_deref());
    }
    let tip = look.map_or_else(FieldLook::name, FieldLook::tool_tip);
    item.setToolTip(Some(&ns(tip)));
}

/// What the switch in `ns_window`'s toolbar shows: its tool tip, or that it is hidden, for the self-test.
#[cfg(feature = "selftest")]
pub(crate) fn field_state(ns_window: &NSWindow) -> String {
    let Some(toolbar) = ns_window.toolbar() else {
        return "no toolbar".into();
    };
    let field = ns(FIELD);
    let Some(item) = toolbar.items().iter().find(|item| *item.itemIdentifier() == *field) else {
        return "no switch".into();
    };
    let selected = item.image().is_some_and(|image| !image.isTemplate());
    format!(
        "{} ({}{})",
        item.toolTip().map(|tip| tip.to_string()).unwrap_or_default(),
        if item.isEnabled() { "enabled" } else { "disabled" },
        if selected { ", selected" } else { "" }
    )
}

/// The toolbar's items with whether each is enabled, and the branch it shows, for the self-test.
#[cfg(feature = "selftest")]
pub(crate) fn state(ns_window: &NSWindow) -> String {
    let Some(toolbar) = ns_window.toolbar() else {
        return "no toolbar".into();
    };
    let items: Vec<String> = toolbar
        .items()
        .iter()
        .map(|item| {
            let id = item.itemIdentifier().to_string();
            let id = id.strip_prefix("term.").unwrap_or(&id).to_string();
            if item.isEnabled() {
                id
            } else {
                format!("{id} (disabled)")
            }
        })
        .collect();
    let branch = delegate_of(ns_window)
        .map(|delegate| {
            let workspace = term_ui::workspace_host::WorkspaceId::new(delegate.ivars().workspace);
            term_ui_git::toolbar_seen(&term_ui::workspace_host::Workspace::new(workspace))
        })
        .and_then(|seen| seen.into_iter().flatten().next())
        .map(|(head, _)| head);
    let new_menu: Vec<String> = delegate_of(ns_window)
        .and_then(|delegate| delegate.ivars().new_menu.borrow().clone())
        .map(|menu| menu.itemArray().iter().map(|item| item.title().to_string()).collect())
        .unwrap_or_default();
    let agents: Vec<String> = toolbar
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string().ends_with("new-agent"))
        .and_then(|item| item.downcast::<NSMenuToolbarItem>().ok())
        .map(|item| {
            let menu = item.menu();
            if let Some(delegate) = menu.delegate() {
                delegate.menuNeedsUpdate(&menu);
            }
            menu.itemArray().iter().map(|item| item.title().to_string()).collect()
        })
        .unwrap_or_default();
    format!(
        "{} | branch {branch:?} | new workspace {new_menu:?} | agents {agents:?}",
        items.join(", ")
    )
}
