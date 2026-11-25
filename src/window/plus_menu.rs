//! The tab bar's `+` (E20): a menu below it of what a new tab beside the tab can show, as Dango's `+` offers it,
//! without its worktree threads and welcome page: a file, a terminal, a browser and the installed agents.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSView, NSWindow};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint};
use splat_i18n::t;
use term_base::guarded;
use term_ui::kit;
use term_ui_agent::Agent;

use super::Window;
use crate::state::try_app;

thread_local! {
    /// The menu shown last with its target, kept until the next: AppKit sends the chosen entry's action after the menu
    /// closed.
    static SHOWN: RefCell<Option<(Retained<NSMenu>, Retained<PlusMenu>)>> = const { RefCell::new(None) };
}

/// Which tab of which window the menu opens beside and the agents its entries stand for, by their tag.
pub(crate) struct Ivars {
    window: u64,
    tab: u64,
    agents: RefCell<Vec<Agent>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `PlusMenu` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermPlusMenu"]
    #[ivars = Ivars]
    pub(crate) struct PlusMenu;

    unsafe impl NSObjectProtocol for PlusMenu {}

    impl PlusMenu {
        #[unsafe(method(termPlusFile:))]
        fn file(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.beside(|window| crate::editor_tab::new_file(window)));
        }

        #[unsafe(method(termPlusTerminal:))]
        fn terminal(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.beside(|window| window.new_tab()));
        }

        #[unsafe(method(termPlusBrowser:))]
        fn browser(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.beside(|window| crate::browser::open(window, None)));
        }

        #[unsafe(method(termPlusAgent:))]
        fn agent(&self, item: &NSMenuItem) {
            guarded((), || {
                let agent = usize::try_from(item.tag())
                    .ok()
                    .and_then(|at| self.ivars().agents.borrow().get(at).cloned());
                if let Some(agent) = agent {
                    self.beside(move |window| crate::agent::open(window, agent));
                }
            });
        }
    }
);

impl PlusMenu {
    fn beside(&self, act: impl FnOnce(&Rc<Window>)) {
        let Ivars { window, tab, .. } = self.ivars();
        let Some(window) = try_app().and_then(|app| app.window(*window)) else {
            return;
        };
        if window.select_beside(*tab).is_some() {
            act(&window);
        }
    }
}

/// Opens the menu of the `+` of `ns_window`, tab `tab` of window `window`, below that `+`.
pub(super) fn open(ns_window: &NSWindow, window: u64, tab: u64) {
    let Some(menu) = prepare(window, tab) else { return };
    let (view, at) = match plus_button(ns_window) {
        Some(button) => {
            let height = button.bounds().size.height;
            (button, NSPoint::new(0.0, height + 4.0))
        }
        None => {
            let Some(content) = ns_window.contentView() else { return };
            let size = content.bounds().size;
            let top = if content.isFlipped() { 0.0 } else { size.height };
            (content, NSPoint::new(size.width / 2.0, top))
        }
    };
    menu.popUpMenuPositioningItem_atLocation_inView(None, at, Some(&view));
}

/// The menu for a new tab beside tab `tab` of window `window`, kept with its target in `SHOWN`.
fn prepare(window: u64, tab: u64) -> Option<Retained<NSMenu>> {
    let mtm = MainThreadMarker::new()?;
    let own = try_app().and_then(|app| app.window(window))?;
    if own.is_quick() {
        objc2_app_kit::NSBeep();
        return None;
    }
    let base = own
        .workspace_of_tab(tab)
        .map_or_else(term_base::home, |workspace| workspace.base.clone());
    let this = PlusMenu::alloc(mtm).set_ivars(Ivars {
        window,
        tab,
        agents: RefCell::new(Vec::new()),
    });
    // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
    let target: Retained<PlusMenu> = unsafe { msg_send![super(this), init] };
    let menu = menu(mtm, &target, &base);
    SHOWN.with(|shown| *shown.borrow_mut() = Some((menu.clone(), target)));
    Some(menu)
}

/// The entries for a new tab in the workspace at `base`; only installed agents the settings offer are listed.
fn menu(mtm: MainThreadMarker, target: &PlusMenu, base: &std::path::Path) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let entry = |title: &str, symbol: &str, action: Option<Sel>| {
        let entry = kit::item(mtm, title, action, "");
        entry.setImage(kit::symbol(symbol, title).as_deref());
        entry.setEnabled(action.is_some());
        kit::set_target(&entry, target as &AnyObject);
        entry
    };
    menu.addItem(&entry(t("Neue Datei"), "doc.badge.plus", Some(sel!(termPlusFile:))));
    menu.addItem(&entry(t("Terminal"), "terminal", Some(sel!(termPlusTerminal:))));
    menu.addItem(&entry(t("Browser"), "globe", Some(sel!(termPlusBrowser:))));
    menu.addItem(&kit::separator(mtm));
    let known = term_ui::workspace_host::try_environment(std::time::Duration::ZERO).is_some();
    let agents: Vec<Agent> = term_ui_agent::agents(base).into_iter().filter(Agent::offered).collect();
    for (at, agent) in agents.iter().enumerate() {
        let symbol = if *agent == Agent::Claude { "sparkle" } else { "sparkles" };
        let item = entry(agent.name(), symbol, Some(sel!(termPlusAgent:)));
        item.setTag(isize::try_from(at).unwrap_or(isize::MAX));
        menu.addItem(&item);
    }
    if !known {
        menu.addItem(&entry(t("Agenten werden gesucht …"), "sparkles", None));
    }
    if agents.is_empty() && known {
        menu.removeItemAtIndex(menu.numberOfItems() - 1);
    }
    *target.ivars().agents.borrow_mut() = agents;
    menu
}

/// The `+` at the end of `window`'s tab bar, AppKit's own button.
fn plus_button(window: &NSWindow) -> Option<Retained<NSView>> {
    fn find(view: &NSView) -> Option<Retained<NSView>> {
        // Key-value observing puts a prefix before the class's name.
        if view.class().name().to_bytes().ends_with(b"NSTabBarNewTabButton") {
            return Some(objc2::Message::retain(view));
        }
        view.subviews().iter().find_map(|child| find(&child))
    }
    // SAFETY: Asking for the superview only reads; the frame view holds the content view while the window shows.
    let frame = window
        .contentView()
        .and_then(|content| unsafe { content.superview() })?;
    find(&frame)
}

/// The entries of the menu of the `+` beside tab `tab` of window `window`, a separator as `---`, a greyed entry in
/// brackets.
#[cfg(feature = "selftest")]
pub(super) fn entries(window: u64, tab: u64) -> Vec<String> {
    let Some(menu) = prepare(window, tab) else {
        return Vec::new();
    };
    menu.itemArray()
        .iter()
        .map(|item| match (item.isSeparatorItem(), item.isEnabled()) {
            (true, _) => "---".into(),
            (false, true) => item.title().to_string(),
            (false, false) => format!("({})", item.title()),
        })
        .collect()
}

/// Chooses the entry titled `title` of the menu of the `+` beside tab `tab` of window `window`; whether there is one.
#[cfg(feature = "selftest")]
pub(super) fn choose(window: u64, tab: u64, title: &str) -> bool {
    let Some(menu) = prepare(window, tab) else { return false };
    let Some(index) = menu
        .itemArray()
        .iter()
        .position(|item| item.title().to_string() == title)
    else {
        return false;
    };
    menu.performActionForItemAtIndex(isize::try_from(index).unwrap_or(0));
    true
}
