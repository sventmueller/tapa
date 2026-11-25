//! The self-test's steps for the menu bar and the app's alerts: `menu:<name>` prints every menu as AppKit validated
//! it, `alert:quit` shows the alert before quitting, has its picture taken and cancels it, and `escape:` posts Escape
//! to the settings window; `tabmenu:<name>` prints the context menu of a tab of the tab bar; `closebutton:<name>`
//! clicks the front window's close button and prints the closing questions it asked, each with the programs it listed;
//! `help:` chooses each item of the Help menu and prints the pages they would open; `about:` chooses About in the
//! app menu, takes the picture `about` of the panel and closes it.

use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenu};
use objc2_foundation::NSPoint;
use term_ui::kit::ns;

thread_local! {
    /// The programs of each closing question since the last `closebutton:`, which the self-test answers yes.
    static ASKED: std::cell::RefCell<Vec<Vec<String>>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Whether the last `closebutton:` would have quit the app, which the self-test leaves running.
    static QUITS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Notes that closing the app's last window would quit it now.
pub(crate) fn quits() {
    QUITS.with(|quits| quits.set(true));
}

/// Notes a closing question listing `busy`, which the self-test agrees to without showing it.
pub(crate) fn asked(busy: &[String]) {
    ASKED.with(|asked| asked.borrow_mut().push(busy.to_vec()));
}

/// Clicks the close button of the front window and prints the closing questions that asked.
pub(super) fn close_button(name: &str) {
    ASKED.with(|asked| asked.borrow_mut().clear());
    QUITS.with(|quits| quits.set(false));
    let button = crate::state::app()
        .front()
        .and_then(|window| window.front_ns_window())
        .and_then(|window| window.standardWindowButton(objc2_app_kit::NSWindowButton::CloseButton));
    let Some(button) = button else {
        println!("closebutton {name}: no close button");
        return;
    };
    // SAFETY: The window's own close button, clicked on the main thread; the click needs no sender.
    unsafe { button.performClick(None) };
    let asked = ASKED.with(std::cell::RefCell::take);
    let quits = if QUITS.with(std::cell::Cell::get) {
        ", quits the app"
    } else {
        ""
    };
    println!("closebutton {name}: {} questions {asked:?}{quits}", asked.len());
}

/// Chooses every item of the Help menu and prints the page each would open in the browser.
pub(super) fn help() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(menu) = NSApplication::sharedApplication(mtm).helpMenu() else {
        return println!("help: no Help menu");
    };
    for at in 0..menu.numberOfItems() {
        let title = menu
            .itemAtIndex(at)
            .map(|item| item.title().to_string())
            .unwrap_or_default();
        menu.performActionForItemAtIndex(at);
        println!("help {title}: {:?}", crate::feedback::take_opened());
    }
}

/// Chooses About in the app menu and takes the picture `about` of the panel it shows.
pub(super) fn about() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let Some(menu) = app
        .mainMenu()
        .and_then(|main| main.itemAtIndex(0))
        .and_then(|item| item.submenu())
    else {
        return;
    };
    let before: Vec<isize> = app.windows().iter().map(|window| window.windowNumber()).collect();
    menu.performActionForItemAtIndex(0);
    let panel = app
        .windows()
        .iter()
        .find(|window| window.isVisible() && !before.contains(&window.windowNumber()));
    let Some(panel) = panel else {
        return println!("about: no panel");
    };
    let dir = std::env::var_os("TAPA_SELFTEST_DIR")
        .map_or_else(|| std::path::PathBuf::from("target/selftest"), std::path::PathBuf::from);
    picture(&panel, &dir.join("about.png"));
    panel.close();
}

/// Prints the context menu of the first tab in the front window's tab bar, as a right click opens it.
pub(super) fn tab_menu(name: &str) {
    let Some(window) = crate::state::app().front().and_then(|window| window.front_ns_window()) else {
        return;
    };
    // SAFETY: Asking for the superview only reads; the frame view holds the content view while the window shows.
    let tab = window
        .contentView()
        .and_then(|view| unsafe { view.superview() })
        .and_then(|frame| tab_button(&frame));
    let Some(tab) = tab else {
        return println!("tabmenu {name}: no tab bar");
    };
    let bounds = tab.bounds();
    let at = tab.convertPoint_toView(NSPoint::new(bounds.size.width / 2.0, bounds.size.height / 2.0), None);
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::RightMouseDown,
        at,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    );
    match event.and_then(|event| tab.menuForEvent(&event)) {
        Some(menu) => {
            println!("tabmenu {name}:");
            print_menu(&menu, 1);
        }
        None => println!("tabmenu {name}: no menu"),
    }
}

/// The first tab of the tab bar under `view`, AppKit's own button.
fn tab_button(view: &objc2_app_kit::NSView) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    // Key-value observing puts a prefix before the class's name.
    if view.class().name().to_bytes().ends_with(b"NSTabButton") {
        return Some(objc2::Message::retain(view));
    }
    view.subviews().iter().find_map(|child| tab_button(&child))
}

/// How long the alert stays up for its picture.
pub(super) const ALERT_TIME: Duration = Duration::from_millis(3000);

/// Prints the menu bar, each item with its shortcut, a `-` where it is disabled, a `✓` where it is ticked, and whether
/// it is hidden or an alternate, which shows only while its modifier key is down.
pub(super) fn print(name: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(main) = NSApplication::sharedApplication(mtm).mainMenu() else {
        return;
    };
    for item in main.itemArray().iter() {
        if let Some(menu) = item.submenu() {
            sideband(&menu);
        }
    }
    println!("menu {name}:");
    print_menu(&main, 0);
}

/// Has AppKit add its own items to `menu`, such as those of window tabs, which it does only as the menu opens.
fn sideband(menu: &NSMenu) {
    let roles = objc2::sel!(_sidebandUpdaterRoles);
    if !objc2_foundation::NSObjectProtocol::respondsToSelector(menu, roles) {
        return;
    }
    // SAFETY: AppKit's own methods of the menu, checked above to exist: a bit set of roles, and one role's update.
    unsafe {
        let roles: isize = objc2::msg_send![menu, _sidebandUpdaterRoles];
        for bit in 0..isize::BITS - 1 {
            if roles & (1 << bit) != 0 {
                let _: () = objc2::msg_send![menu, _performSidebandUpdatersForRole: 1isize << bit];
            }
        }
    }
}

fn print_menu(menu: &NSMenu, depth: usize) {
    menu.update();
    for item in menu.itemArray().iter() {
        if item.isSeparatorItem() {
            println!("menu {}---", "  ".repeat(depth));
            continue;
        }
        let key = item.keyEquivalent().to_string();
        let flags = item.keyEquivalentModifierMask();
        let mut chord = String::new();
        for (flag, glyph) in [
            (NSEventModifierFlags::Function, "fn"),
            (NSEventModifierFlags::Control, "⌃"),
            (NSEventModifierFlags::Option, "⌥"),
            (NSEventModifierFlags::Shift, "⇧"),
            (NSEventModifierFlags::Command, "⌘"),
        ] {
            if !key.is_empty() && flags.contains(flag) {
                chord.push_str(glyph);
            }
        }
        chord.push_str(&key.escape_debug().to_string());
        println!(
            "menu {}{}{}{}{} {}",
            "  ".repeat(depth),
            match (item.isHidden(), item.isAlternate()) {
                (true, _) => "(hidden) ",
                (false, true) => "(alternate) ",
                (false, false) => "",
            },
            if item.isEnabled() { "" } else { "-" },
            if item.state() == objc2_app_kit::NSControlStateValueOn {
                "✓"
            } else {
                ""
            },
            item.title(),
            chord
        );
        if let Some(submenu) = item.submenu() {
            print_menu(&submenu, depth + 1);
        }
    }
}

/// Shows the alert `name` asks, takes its picture `alert-<name>` and cancels it.
pub(super) fn alert(name: &str) {
    if name != "quit" {
        eprintln!("selftest: no alert {name}");
        return;
    }
    let shot = format!("alert-{name}");
    term_base::main_after(Duration::from_millis(1500), move || {
        let Some(mtm) = MainThreadMarker::new() else { return };
        if let Some(window) = NSApplication::sharedApplication(mtm).modalWindow() {
            let dir = std::env::var_os("TAPA_SELFTEST_DIR")
                .map_or_else(|| std::path::PathBuf::from("target/selftest"), std::path::PathBuf::from);
            picture(&window, &dir.join(format!("{shot}.png")));
        }
    });
    term_base::main_after(ALERT_TIME - Duration::from_millis(500), || {
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).abortModal();
        }
    });
    term_base::main_thread::main_on_run_loop(|| {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let agreed = crate::quit::ask(
            mtm,
            &splat_i18n::tf("{} beenden?", &[term_product::NAME]),
            &splat_i18n::tf("Diese Programme laufen noch und werden beendet: {}", &["vim, cargo"]),
            splat_i18n::t("Beenden"),
        );
        println!("alert quit: agreed {agreed}");
    });
}

/// Posts Escape to the settings window, through the app's event queue as a key press goes.
pub(super) fn escape() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(window) = crate::settings::ns_window() else {
        println!("escape: no settings window");
        return;
    };
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::ZERO,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        &ns("\u{1b}"),
        &ns("\u{1b}"),
        false,
        0x35,
    );
    if let Some(event) = event {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
    term_base::main_after(Duration::from_millis(300), || {
        println!(
            "escape: settings window open {}",
            crate::settings::ns_window().is_some()
        );
    });
}

/// Draws `window`'s content into a PNG at `path`; the window server leaves an alert's glass blank in a screenshot of
/// a process in the background.
pub(super) fn picture(window: &objc2_app_kit::NSWindow, path: &std::path::Path) {
    use objc2_app_kit::NSBitmapImageFileType;
    let Some(view) = window.contentView() else { return };
    let bounds = view.bounds();
    let Some(bitmap) = view.bitmapImageRepForCachingDisplayInRect(bounds) else {
        return;
    };
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
    // SAFETY: An empty dictionary of properties, which PNG needs none of.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new())
    };
    let written = png.is_some_and(|png| std::fs::write(path, png.to_vec()).is_ok());
    println!("alert picture {}: {written}", path.display());
}
