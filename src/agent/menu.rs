//! The toolbar's New Agent Session button (E21, E22): a menu of the agents a new tab can run in the selected
//! workspace, those whose program is missing greyed; while only one of them is installed, a click opens it without the
//! menu.

use std::cell::RefCell;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEventType, NSMenu, NSMenuDelegate, NSMenuItem, NSMenuToolbarItem, NSToolbarItem};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString};
use splat_i18n::t;
use term_base::guarded;
use term_ui::kit::{self, ns};
use term_ui_agent::Agent;

use crate::state::try_app;

/// Which window the menu belongs to, the agents its items stand for, by their tag, and its button.
pub(crate) struct Ivars {
    window: u64,
    agents: RefCell<Vec<Agent>>,
    button: Retained<NSMenuToolbarItem>,
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

        #[unsafe(method(termNewOnlyAgent:))]
        fn new_only_agent(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.clicked());
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
        let item = NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), identifier);
        let this = AgentMenu::alloc(mtm).set_ivars(Ivars {
            window,
            agents: RefCell::new(Vec::new()),
            button: item.clone(),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        let delegate: Retained<AgentMenu> = unsafe { msg_send![super(this), init] };
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
        let target: &AnyObject = &delegate;
        // SAFETY: The delegate answers the button's action and lives as long as the button.
        unsafe { item.setTarget(Some(target)) };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        // AppKit never opens an empty menu, so it would never ask the delegate to fill it.
        delegate.fill(&menu);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        item.setMenu(&menu);
        (Retained::into_super(item), delegate)
    }

    /// Has a click on the button open the only installed agent on offer for the window's selected workspace, else
    /// show the menu.
    pub(crate) fn follow(&self) {
        self.follow_offered(&super::offered(&self.base()));
    }

    /// The agent a click opens without the menu, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn click_opens(&self) -> Option<String> {
        self.ivars().button.action().map(|_| {
            let agents = super::offered(&self.base());
            only(&agents).map_or_else(|| "?".into(), |agent| agent.name().to_string())
        })
    }

    fn follow_offered(&self, offered: &[(Agent, bool)]) {
        let action = only(offered).map(|_| sel!(termNewOnlyAgent:));
        let button = &self.ivars().button;
        if button.action() != action {
            // Without an action AppKit shows the menu on a click; with one the click sends it.
            // SAFETY: The button's target is this object, which answers that action and lives as long as the button.
            unsafe { button.setAction(action) };
        }
    }

    /// A click while the button stood for one agent: opens it if it still is the only one, else shows the menu there.
    fn clicked(&self) {
        let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) else {
            return;
        };
        let offered = super::offered(&self.base());
        if let Some(agent) = only(&offered) {
            return super::open(&window, agent);
        }
        self.follow_offered(&offered);
        self.pop_up();
    }

    /// Shows the button's menu at the mouse, or at the top of the window when the click came from elsewhere.
    fn pop_up(&self) {
        let mtm = self.mtm();
        let event = NSApplication::sharedApplication(mtm).currentEvent();
        let mouse = event
            .as_ref()
            .filter(|event| matches!(event.r#type(), NSEventType::LeftMouseDown | NSEventType::LeftMouseUp));
        let ns_window = mouse
            .and_then(|event| event.window(mtm))
            .or_else(|| try_app()?.window(self.ivars().window)?.front_ns_window());
        let Some(view) = ns_window.and_then(|ns_window| ns_window.contentView()) else {
            return;
        };
        let at = match mouse {
            Some(event) => view.convertPoint_fromView(event.locationInWindow(), None),
            None => {
                let size = view.bounds().size;
                NSPoint::new(size.width / 2.0, if view.isFlipped() { 0.0 } else { size.height })
            }
        };
        self.ivars()
            .button
            .menu()
            .popUpMenuPositioningItem_atLocation_inView(None, at, Some(&view));
    }

    fn base(&self) -> PathBuf {
        try_app()
            .and_then(|app| app.window(self.ivars().window))
            .and_then(|window| window.selected_workspace())
            .map_or_else(term_base::home, |workspace| workspace.base.clone())
    }

    /// Lists the agents for the window's selected workspace.
    fn fill(&self, menu: &NSMenu) {
        let offered = super::offered(&self.base());
        self.follow_offered(&offered);
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
        let target: &AnyObject = self;
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

/// The agent a click opens without the menu: the only one on offer whose program is installed.
fn only(offered: &[(Agent, bool)]) -> Option<Agent> {
    let mut installed = offered.iter().filter(|(_, installed)| *installed);
    match (installed.next(), installed.next()) {
        (Some((agent, _)), None) => Some(agent.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_single_installed_agent_skips_the_menu() {
        let claude_only = [(Agent::Claude, true), (Agent::Codex, false), (Agent::OpenCode, false)];
        assert_eq!(only(&claude_only), Some(Agent::Claude));
        let two = [(Agent::Claude, true), (Agent::Codex, true), (Agent::OpenCode, false)];
        assert_eq!(only(&two), None, "two installed agents leave the choice to the menu");
        let none = [(Agent::Claude, false), (Agent::Codex, false)];
        assert_eq!(only(&none), None, "nothing installed shows the greyed menu");
    }
}
