//! The toolbar's New Agent Session button (E21, E22): a menu of the agents a new tab can run in the selected
//! workspace, those whose program is missing greyed.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuDelegate, NSMenuItem, NSMenuToolbarItem, NSToolbarItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use splat_i18n::t;
use term_base::guarded;
use term_ui::kit::{self, ns};
use term_ui_agent::Agent;

use crate::state::try_app;

/// Which window the menu belongs to and the agents its items stand for, by their tag.
pub(crate) struct Ivars {
    window: u64,
    agents: RefCell<Vec<Agent>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AgentMenu` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentMenu"]
    #[ivars = Ivars]
    pub(crate) struct AgentMenu;

    unsafe impl NSObjectProtocol for AgentMenu {}

    unsafe impl NSMenuDelegate for AgentMenu {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            guarded((), || self.fill(menu));
        }
    }

    impl AgentMenu {
        #[unsafe(method(termNewAgent:))]
        fn new_agent(&self, item: &NSMenuItem) {
            guarded((), || {
                let agent = usize::try_from(item.tag())
                    .ok()
                    .and_then(|at| self.ivars().agents.borrow().get(at).cloned());
                let window = try_app().and_then(|app| app.window(self.ivars().window));
                if let (Some(agent), Some(window)) = (agent, window) {
                    super::open(&window, agent);
                }
            });
        }
    }
);

impl AgentMenu {
    /// The toolbar item of window `window` with its menu, and the menu's delegate, which must live as long as the
    /// item.
    pub(crate) fn item(
        mtm: MainThreadMarker,
        identifier: &NSString,
        window: u64,
    ) -> (Retained<NSToolbarItem>, Retained<AgentMenu>) {
        let this = AgentMenu::alloc(mtm).set_ivars(Ivars {
            window,
            agents: RefCell::new(Vec::new()),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        let delegate: Retained<AgentMenu> = unsafe { msg_send![super(this), init] };
        let item = NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), identifier);
        let title = t("Neue Agent-Sitzung");
        let label = ns(title);
        item.setImage(kit::symbol("sparkles", title).as_deref());
        item.setLabel(&label);
        item.setPaletteLabel(&label);
        item.setToolTip(Some(&label));
        item.setShowsIndicator(false);
        // Agents not installed only grey their items; the button itself stays, also before the login shell's PATH is
        // known, which decides what counts as installed.
        item.setAutovalidates(false);
        item.setEnabled(true);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        // AppKit never opens an empty menu, so it would never ask the delegate to fill it.
        delegate.fill(&menu);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        item.setMenu(&menu);
        (Retained::into_super(item), delegate)
    }

    /// Lists the agents for the window's selected workspace.
    fn fill(&self, menu: &NSMenu) {
        let base = try_app()
            .and_then(|app| app.window(self.ivars().window))
            .and_then(|window| window.selected_workspace())
            .map_or_else(term_base::home, |workspace| workspace.base.clone());
        let offered = super::offered(&base);
        let shown: Vec<(String, bool)> = menu
            .itemArray()
            .iter()
            .map(|item| (item.title().to_string(), item.isEnabled()))
            .collect();
        let wanted: Vec<(String, bool)> = offered
            .iter()
            .map(|(agent, installed)| (agent.name().to_string(), *installed))
            .collect();
        // AppKit asks again while an item is being chosen; rebuilding then would drop the chosen item's action.
        if !shown.is_empty() && shown == wanted {
            return;
        }
        menu.removeAllItems();
        let target: &objc2::runtime::AnyObject = self;
        for (at, (agent, installed)) in offered.iter().enumerate() {
            let item = kit::item(self.mtm(), agent.name(), Some(sel!(termNewAgent:)), "");
            item.setTag(isize::try_from(at).unwrap_or(isize::MAX));
            item.setEnabled(*installed);
            if !installed {
                item.setToolTip(Some(&ns(t("Das Programm ist nicht installiert."))));
            }
            kit::set_target(&item, target);
            menu.addItem(&item);
        }
        *self.ivars().agents.borrow_mut() = offered.into_iter().map(|(agent, _)| agent).collect();
    }
}
