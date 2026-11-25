//! The main menu, built from the key bindings of the settings (F167), and the key monitor that runs every binding,
//! also a chord bound beside the one the menu shows; each action goes to the window in front.

mod bar;
mod zoom;

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSApplication, NSEvent, NSEventMask, NSMenu, NSMenuItem};
use splat_i18n::t;
use term_config::{Action, Binding};
use term_ui::kit::{item, separator, submenu};
use term_ui_panes::{Direction, Side};

use crate::app::AppDelegate;
use crate::state::{app, try_app};
use bar::Entry;

thread_local! {
    static DELEGATE: RefCell<Option<Retained<AppDelegate>>> = const { RefCell::new(None) };
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// Builds the main menu and starts the key monitor.
pub(crate) fn install(app: &NSApplication, delegate: &Retained<AppDelegate>, mtm: MainThreadMarker) {
    DELEGATE.with(|cell| *cell.borrow_mut() = Some(delegate.clone()));
    app.setMainMenu(Some(&build(mtm)));
    monitor();
}

/// Builds the main menu again, after the key bindings changed.
pub(crate) fn rebuild(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&build(mtm)));
}

/// The title of `action` in the menu: the one the settings and the palette show, or for an item that shows or hides
/// something, what choosing it does now.
pub(crate) fn title(action: Action) -> String {
    let shown = |show: &str, hide: &str, on: Option<bool>| on.map(|on| if on { hide } else { show }.to_string());
    let dynamic = match action {
        Action::ToggleSidebar => shown(
            t("Seitenleiste einblenden"),
            t("Seitenleiste ausblenden"),
            try_app()
                .and_then(|app| app.front())
                .map(|window| sidebar_shown(&window)),
        ),
        Action::ToggleInputField => shown(
            t("Eingabefeld einblenden"),
            t("Eingabefeld ausblenden"),
            try_app()
                .and_then(|app| app.front())
                .and_then(|window| window.focused_view())
                .map(|view| view.field_shown()),
        ),
        Action::ToggleTrust => shown(
            t("Arbeitsbereich vertrauen"),
            t("Vertrauen entziehen"),
            try_app()
                .and_then(|app| app.front())
                .and_then(|window| window.selected_workspace())
                .map(|workspace| workspace.is_trusted()),
        ),
        Action::ToggleDictation => shown(t("Diktat starten"), t("Diktat beenden"), crate::dictation::recording()),
        Action::QuickTerminal => shown(
            t("Schnelles Terminal einblenden"),
            t("Schnelles Terminal ausblenden"),
            try_app().map(|_| crate::quick::in_front()),
        ),
        _ => None,
    };
    dynamic.unwrap_or_else(|| crate::settings::action_title(action))
}

/// Whether `window` shows the sidebar.
fn sidebar_shown(window: &crate::window::Window) -> bool {
    window.sidebar_shown()
}

/// Whether the item of `action` can run now, and its title and tick as things are; the app delegate's validation.
pub(crate) fn validate(item: &NSMenuItem) -> bool {
    if item.action() != Some(sel!(termAction:)) {
        return true;
    }
    let Some(&action) = usize::try_from(item.tag()).ok().and_then(|tag| Action::ALL.get(tag)) else {
        return false;
    };
    item.setTitle(&term_ui::kit::ns(&title(action)));
    if action == Action::SecureKeyboardEntry {
        item.setState(if term_ui::secure::manual() {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
    }
    if action == Action::ToggleDictation {
        item.setHidden(!crate::dictation::BUILT);
    }
    if matches!(
        action,
        Action::NewClaudeTab | Action::NewCodexTab | Action::NewOpenCodeTab
    ) {
        item.setHidden(crate::agent::switched_off(action));
    }
    let Some(app) = try_app() else { return false };
    let front = app.front();
    match action {
        Action::ToggleDictation => crate::dictation::can_toggle(app.mtm),
        Action::NewTab | Action::NewWorkspace => front.is_none_or(|window| !window.is_quick()),
        Action::RenameWorkspace | Action::CloseWorkspace => front.is_some_and(|window| {
            !window.is_quick() && window.selected_workspace().is_some_and(|workspace| !workspace.scratch)
        }),
        Action::NextWorkspace
        | Action::PreviousWorkspace
        | Action::ToggleTrust
        | Action::NewBrowserTab
        | Action::NewFile
        | Action::OpenFile
        | Action::ToggleFiles
        | Action::ToggleGit => front.is_some_and(|window| !window.is_quick()),
        Action::SaveAll => term_ui_editor::has_unsaved(),
        Action::NewClaudeTab | Action::NewCodexTab | Action::NewOpenCodeTab => {
            front.is_some_and(|window| !window.is_quick() && crate::agent::installed(&window, action))
        }
        Action::NewWindow
        | Action::Settings
        | Action::ReloadConfig
        | Action::SecureKeyboardEntry
        | Action::QuickTerminal => true,
        Action::Close | Action::CloseTab | Action::CloseWindow => front.is_some() || crate::settings::is_key(),
        Action::ToggleSidebar => front.is_some_and(|window| !window.is_quick()),
        Action::CommandPalette
        | Action::TabSwitcher
        | Action::SplitRight
        | Action::SplitDown
        | Action::ClosePane
        | Action::ZoomPane
        | Action::EqualizePanes
        | Action::ResizeLeft
        | Action::ResizeRight
        | Action::ResizeUp
        | Action::ResizeDown
        | Action::NextPane
        | Action::PreviousPane
        | Action::NextTab
        | Action::PreviousTab
        | Action::Tab1
        | Action::Tab2
        | Action::Tab3
        | Action::Tab4
        | Action::Tab5
        | Action::Tab6
        | Action::Tab7
        | Action::Tab8
        | Action::LastTab => front.is_some(),
        Action::FontBigger | Action::FontSmaller | Action::FontReset => {
            front.is_some_and(|window| zoom::can_run(&window))
        }
        _ => front.and_then(|window| window.focused_view()).is_some(),
    }
}

fn bindings() -> Vec<Binding> {
    let (bindings, _) = app().config().bindings();
    bindings
}

/// The first chord bound to `action`, as a menu writes it.
fn chord_for(bindings: &[Binding], action: Action) -> String {
    bindings
        .iter()
        .find(|binding| binding.action == action)
        .map(|binding| binding.chord.to_string())
        .unwrap_or_default()
}

/// A menu item that runs `action` through the app delegate.
fn action_item(mtm: MainThreadMarker, bindings: &[Binding], action: Action) -> Retained<NSMenuItem> {
    let entry = item(
        mtm,
        &title(action),
        Some(sel!(termAction:)),
        &chord_for(bindings, action),
    );
    let tag = Action::ALL.iter().position(|a| *a == action).unwrap_or(0);
    entry.setTag(isize::try_from(tag).unwrap_or(0));
    DELEGATE.with(|cell| {
        if let Some(delegate) = cell.borrow().as_ref() {
            term_ui::kit::set_target(&entry, delegate);
        }
    });
    entry
}

/// The items of `entries`, AppKit's services menu among them if there is one.
fn items(mtm: MainThreadMarker, bindings: &[Binding], entries: Vec<Entry>) -> Vec<Retained<NSMenuItem>> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Action(action) => out.push(action_item(mtm, bindings, action)),
            Entry::Standard(title, selector, key) => out.push(item(mtm, &title, Some(selector), key)),
            Entry::Bound(title, selector, action) => {
                out.push(item(mtm, &title, Some(selector), &chord_for(bindings, action)));
            }
            Entry::Item(make) => out.push(make(mtm, &editor_shortcut)),
            Entry::Updates => out.extend(crate::updates::menu_item(mtm)),
            Entry::Services => {
                let (services, menu) = submenu(mtm, t("Dienste"), Vec::new());
                NSApplication::sharedApplication(mtm).setServicesMenu(Some(&menu));
                out.push(services);
            }
            Entry::Separator => out.push(separator(mtm)),
            Entry::Hidden(selector) => {
                let hidden = item(mtm, "", Some(selector), "");
                hidden.setHidden(true);
                out.push(hidden);
            }
            Entry::Menu(title, inner) => out.push(submenu(mtm, &title, items(mtm, bindings, inner)).0),
        }
    }
    out
}

/// The shortcut of an editor command by its name in Dango's keymap; Dango's ⌘I and ⌘K are the input field's and
/// Clear's here.
fn editor_shortcut(name: &str) -> String {
    match name {
        "format.bold" => "cmd+b",
        "edit.pastePlain" => "alt+shift+cmd+v",
        _ => "",
    }
    .to_string()
}

/// The main menu: the app's menus with the editor's Format menu after Edit and the Git menu after View (E22).
fn build(mtm: MainThreadMarker) -> Retained<NSMenu> {
    let b = bindings();
    let main = NSMenu::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    let menus = bar::layout(term_product::NAME);
    let count = menus.len();
    for (at, (title, entries)) in menus.into_iter().enumerate() {
        let (entry, menu) = submenu(mtm, &title, items(mtm, &b, entries));
        main.addItem(&entry);
        if at + 2 == count {
            app.setWindowsMenu(Some(&menu));
        } else if at + 1 == count {
            app.setHelpMenu(Some(&menu));
        } else if at == 2 {
            main.addItem(&term_ui_editor::format_menu(mtm, &editor_shortcut));
        } else if at == 3 {
            main.addItem(&term_ui_git::menu(mtm));
        }
    }
    main
}

/// Runs the binding of every key press in the app's windows before the view sees it, so that each chord the
/// settings bind works, not only the one its menu item shows.
fn monitor() {
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands the monitor a live event for the duration of the call.
        let event_ref = unsafe { event.as_ref() };
        let handled = term_base::guarded(false, || handle(event_ref));
        if handled { std::ptr::null_mut() } else { event.as_ptr() }
    });
    // SAFETY: The block returns the event or null, as a local monitor must, and runs on the main thread.
    let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) };
    MONITOR.with(|cell| *cell.borrow_mut() = token);
}

fn handle(event: &NSEvent) -> bool {
    if crate::settings::escape(event) {
        return true;
    }
    let Some(app) = try_app() else { return false };
    let Some(window) = event.window(app.mtm) else {
        return false;
    };
    if !app.windows.borrow().iter().any(|own| own.owns(&window)) {
        return false;
    }
    let Some(chord) = term_ui::chord_of(event) else {
        return false;
    };
    let (bindings, _) = app.config().bindings();
    let Some(action) = bindings
        .iter()
        .find(|binding| binding.chord == chord)
        .map(|binding| binding.action)
    else {
        return false;
    };
    let own = app.windows.borrow().iter().find(|own| own.owns(&window)).cloned();
    let in_terminal = own.as_ref().is_some_and(|own| own.focused_view().is_some());
    if acts_on_session(action) && !in_terminal {
        return false;
    }
    let zooms = matches!(action, Action::FontBigger | Action::FontSmaller | Action::FontReset);
    if zooms && !own.is_some_and(|own| zoom::can_run(&own)) {
        return false;
    }
    perform(action);
    true
}

/// Whether `action` acts on the terminal session in front, so that in an editor, a browser or an agent tab its chord
/// goes to that view instead, as ⌘↑ moves to the top of a text.
fn acts_on_session(action: Action) -> bool {
    matches!(
        action,
        Action::Copy
            | Action::CopyFormatted
            | Action::Paste
            | Action::SelectAll
            | Action::Clear
            | Action::Find
            | Action::FindNext
            | Action::FindPrevious
            | Action::PreviousPrompt
            | Action::NextPrompt
            | Action::SelectBlock
            | Action::CopyOutput
            | Action::SaveOutput
            | Action::ToggleBlock
            | Action::ToggleBookmark
            | Action::PreviousBookmark
            | Action::NextBookmark
            | Action::ScrollTop
            | Action::ScrollBottom
            | Action::PageUp
            | Action::PageDown
            | Action::ToggleInputField
            | Action::CopyMode
    )
}

/// Runs `action` on the window in front.
pub(crate) fn perform(action: Action) {
    if crate::settings::handles(action) {
        return;
    }
    let app = app();
    let mtm = app.mtm;
    let send = |selector: Sel| {
        let shared = NSApplication::sharedApplication(mtm);
        // SAFETY: The selectors sent are actions that take the sender, which is none here.
        if unsafe { shared.sendAction_to_from(selector, None, None) } {
            return;
        }
        // Without a key window, as while another app is in front, the responder chain is empty.
        if let Some(view) = app.front().and_then(|window| window.focused_view()) {
            let target: &AnyObject = &view;
            // SAFETY: As above; the terminal view answers every selector sent here.
            unsafe { shared.sendAction_to_from(selector, Some(target), None) };
        }
    };
    match action {
        Action::NewWindow => {
            let folder = app
                .front()
                .and_then(|front| front.selected_workspace())
                .map(|workspace| workspace.base.clone());
            let config = app.config();
            let folder = crate::window::start_folder(&config, folder, config.window.inherit_directory.window);
            crate::window::Window::open(Some(folder), None);
            return;
        }
        Action::Settings => return crate::settings::show(),
        Action::QuickTerminal => return crate::quick::toggle(),
        Action::CommandPalette => return crate::palette::toggle(),
        Action::TabSwitcher => return crate::switcher::toggle(),
        Action::ReloadConfig => return crate::state::reload(),
        Action::SecureKeyboardEntry => return term_ui::secure::set_manual(!term_ui::secure::manual()),
        Action::Copy => return send(sel!(copy:)),
        Action::Paste => return send(sel!(paste:)),
        Action::SelectAll => return send(sel!(selectAll:)),
        Action::Clear => return send(sel!(termClear:)),
        Action::Save => return send(sel!(termSave:)),
        Action::SaveAs => return send(sel!(termSaveAs:)),
        Action::SaveAll => return crate::editor_tab::save_all(),
        Action::ToggleDictation => return crate::dictation::toggle(mtm),
        _ => {}
    }
    let Some(window) = app.front() else {
        if matches!(action, Action::NewTab | Action::NewWorkspace) {
            crate::window::Window::open(None, None);
        }
        return;
    };
    on_window(&app, &window, action);
}

/// Runs `action`, one that acts on a window, on `window`.
fn on_window(app: &crate::state::App, window: &std::rc::Rc<crate::window::Window>, action: Action) {
    match action {
        Action::NewTab => window.new_tab(),
        Action::NewWorkspace => window.new_workspace(),
        Action::RenameWorkspace => window.rename_workspace(None),
        Action::NewClaudeTab | Action::NewCodexTab | Action::NewOpenCodeTab => crate::agent::perform(window, action),
        Action::NewBrowserTab => crate::browser::open(window, None),
        Action::NewFile => crate::editor_tab::new_file(window),
        Action::OpenFile => crate::editor_tab::ask_and_open(window),
        Action::ToggleFiles => window.toggle_selected_panel(term_ui_sidebar::Panel::Files),
        Action::ToggleGit => window.toggle_selected_panel(term_ui_sidebar::Panel::Git),
        Action::ToggleTrust => window.toggle_trust(),
        Action::CloseWorkspace => {
            if let Some(workspace) = window.selected_workspace() {
                window.close_workspace(workspace.id);
            }
        }
        Action::NextWorkspace => window.step_workspace(1),
        Action::PreviousWorkspace => window.step_workspace(-1),
        Action::Close => window.close_active(),
        Action::CloseTab => {
            if let Some(tab) = window.selected_tab_id() {
                window.close_tab(tab);
            }
        }
        Action::CloseWindow => window.close_window(),
        Action::SplitRight => window.split(Side::Right),
        Action::SplitDown => window.split(Side::Below),
        Action::ClosePane => window.close_pane(),
        Action::ZoomPane => window.toggle_zoom(),
        Action::EqualizePanes => window.equalize(),
        Action::NextPane => window.step_pane(1),
        Action::PreviousPane => window.step_pane(-1),
        Action::NextTab => window.select_relative(1),
        Action::PreviousTab => window.select_relative(-1),
        Action::Tab1 => window.select_index(Some(0)),
        Action::Tab2 => window.select_index(Some(1)),
        Action::Tab3 => window.select_index(Some(2)),
        Action::Tab4 => window.select_index(Some(3)),
        Action::Tab5 => window.select_index(Some(4)),
        Action::Tab6 => window.select_index(Some(5)),
        Action::Tab7 => window.select_index(Some(6)),
        Action::Tab8 => window.select_index(Some(7)),
        Action::LastTab => window.select_index(None),
        Action::ToggleSidebar => window.toggle_sidebar(),
        Action::ResizeLeft => window.resize_pane(Direction::Left),
        Action::ResizeRight => window.resize_pane(Direction::Right),
        Action::ResizeUp => window.resize_pane(Direction::Up),
        Action::ResizeDown => window.resize_pane(Direction::Down),
        Action::FontBigger | Action::FontSmaller | Action::FontReset => zoom::run(app, window, action),
        _ => on_view(window.focused_view().as_deref(), action),
    }
}

/// Runs `action`, one that acts on the session in front, on `view`.
fn on_view(view: Option<&term_ui::TerminalView>, action: Action) {
    let scroll = |scroll: term_session::Scroll| {
        if let Some(view) = view {
            view.with_session(|session| session.scroll(scroll));
            view.refresh_now();
        }
    };
    match action {
        Action::ScrollTop => scroll(term_session::Scroll::Top),
        Action::ScrollBottom => scroll(term_session::Scroll::Bottom),
        Action::PageUp => scroll(term_session::Scroll::PageUp),
        Action::PageDown => scroll(term_session::Scroll::PageDown),
        Action::PreviousPrompt => {
            if let Some(view) = view
                && !view.jump_to_prompt(-1)
            {
                view.notify(t("Kein früherer Prompt (Prompts kennt nur der eigene Terminal-Kern)."));
            }
        }
        Action::NextPrompt => {
            if let Some(view) = view {
                view.jump_to_prompt(1);
            }
        }
        Action::SelectBlock
        | Action::CopyOutput
        | Action::SaveOutput
        | Action::ToggleBlock
        | Action::ToggleBookmark
        | Action::PreviousBookmark
        | Action::NextBookmark => {
            if let Some(view) = view {
                block_action(view, action);
            }
        }
        Action::Find => {
            if let Some(view) = view {
                view.open_find();
            }
        }
        Action::FindNext | Action::FindPrevious => {
            if let Some(view) = view {
                view.find_again(action == Action::FindNext);
            }
        }
        Action::CopyMode => {
            if let Some(view) = view {
                view.toggle_copy_mode();
            }
        }
        Action::CopyFormatted => {
            if let Some(view) = view {
                view.copy_formatted();
            }
        }
        Action::ToggleInputField => {
            if let Some(view) = view {
                crate::window::toggle_field(view);
            }
        }
        _ => {}
    }
}

/// Runs `action`, one of the command blocks' and bookmarks' (F78, F81, F87), on `view`.
fn block_action(view: &term_ui::TerminalView, action: Action) {
    match action {
        Action::SelectBlock => {
            view.select_block(None);
        }
        Action::CopyOutput => {
            view.copy_block_output(None);
        }
        Action::SaveOutput => {
            view.save_block_output(None);
        }
        Action::ToggleBlock => {
            view.toggle_block(None);
        }
        Action::ToggleBookmark => {
            view.toggle_bookmark();
        }
        Action::PreviousBookmark | Action::NextBookmark => {
            let step = if action == Action::NextBookmark { 1 } else { -1 };
            if !view.jump_to_bookmark(step) && step < 0 {
                view.notify(t("Kein früheres Lesezeichen."));
            }
        }
        _ => {}
    }
}
