//! The self-test's steps for workspaces and tabs (E20): `sidebar:<name>` prints the workspaces of the window in front,
//! the selected one marked `*`, each with its tabs below; `tabbar:<name>` prints whether the native tab bar of the
//! window in front shows and the titles of its tabs; `tabs:<name>` prints every tab group of the window, on screen or
//! off it, with its workspace, frame, sidebar and tabs; `tabout:` moves the tab in front into a window of its own,
//! `mergeall` merges the windows of its workspace again; `sidebarwidth:<points>` drags the sidebar of the window in
//! front to that width, `windowsize:<width>x<height>` resizes that window as a drag at its edge does; `row:<new|close|rename>.<index>` does what the menu of the sidebar's row `<index>` does;
//! `workspace:<path>` opens a workspace in that folder as the panel of New Workspace would; `rename:<index>=<name>`
//! names a workspace, `rowmove:<from>.<to>` drags a sidebar row to another place as the mouse does; `tabmove:<from>.<to>` drags a tab of the front window's tab bar to another place;
//! `gear:` clicks the settings button at the foot of the sidebar in front; `fullscreen:<name>` prints how the sidebar
//! sits below the toolbar in full screen, `togglefullscreen` enters or leaves full screen, `toolbarslide:<points>` slides the toolbar's window down as
//! the menu bar does;
//! `plusmenu:<name>` prints the entries of the tab bar's `+` menu, `plus:<title>` chooses one;
//! `toolbar:<name>` prints the toolbar's items and branch; `recent:<name>` the folders New Workspace offers; `sheetshot:<name>` takes a picture of the sheet
//! on the window in front and `sheetclose:` cancels it.

use term_ui_sidebar::RowAction;

use crate::state::app;
use crate::tab::TabContent;

/// Runs `step` if it is one of these; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some(window) = app().front() else {
        return false;
    };
    if let Some(name) = step.strip_prefix("sidebar:") {
        print_sidebar(&window, name);
    } else if let Some(name) = step.strip_prefix("tabbar:") {
        println!("tabbar {name}: {}", window.tab_bar_state());
    } else if let Some(name) = step.strip_prefix("tabs:") {
        for line in window.groups_state() {
            println!("tabs {name}: {line}");
        }
    } else if step == "tabout:" {
        if let Some(ns_window) = window.front_ns_window() {
            ns_window.moveTabToNewWindow(None);
        }
    } else if step == "mergeall" || step == "mergeall:" {
        if let Some(ns_window) = window.front_ns_window() {
            ns_window.mergeAllWindows(None);
        }
    } else if let Some(width) = step.strip_prefix("sidebarwidth:") {
        window.set_sidebar_width(width.parse().unwrap_or(230.0));
    } else if let Some(size) = step.strip_prefix("windowsize:") {
        let (width, height) = size.split_once('x').unwrap_or((size, "600"));
        window.set_size(width.parse().unwrap_or(900.0), height.parse().unwrap_or(600.0));
    } else if let Some(name) = step.strip_prefix("fullscreen:") {
        println!("fullscreen {name}: {}", window.full_screen_state());
    } else if step == "togglefullscreen" {
        if let Some(ns_window) = window.front_ns_window() {
            ns_window.toggleFullScreen(None);
        }
    } else if let Some(by) = step.strip_prefix("toolbarslide:") {
        window.slide_toolbar(by.parse().unwrap_or(0.0));
    } else if step == "gear:" {
        println!("gear: {:?}", window.press_settings());
    } else if let Some(name) = step.strip_prefix("plusmenu:") {
        println!("plusmenu {name}: {}", window.plus_menu_entries().join(" | "));
    } else if let Some(title) = step.strip_prefix("plus:") {
        println!("plus {title}: {}", window.choose_plus_entry(title));
    } else if let Some(name) = step.strip_prefix("toolbar:") {
        println!("toolbar {name}: {}", window.toolbar_state());
    } else if let Some(name) = step.strip_prefix("sheetshot:") {
        let sheet = window.front_ns_window().and_then(|ns_window| ns_window.attachedSheet());
        match sheet {
            Some(sheet) => {
                let dir = std::env::var_os("TAPA_SELFTEST_DIR")
                    .map_or_else(|| std::path::PathBuf::from("target/selftest"), std::path::PathBuf::from);
                println!(
                    "shot {} {}",
                    sheet.windowNumber(),
                    dir.join(format!("{name}.png")).display()
                );
            }
            None => println!("sheetshot {name}: no sheet"),
        }
    } else if step == "sheetclose:" {
        if let Some(ns_window) = window.front_ns_window()
            && let Some(sheet) = ns_window.attachedSheet()
        {
            ns_window.endSheet_returnCode(&sheet, objc2_app_kit::NSModalResponseCancel);
        }
    } else if let Some(name) = step.strip_prefix("recent:") {
        println!("recent {name}: {:?}", crate::recent::folders());
    } else if let Some(spec) = step.strip_prefix("row:") {
        let (action, index) = spec.split_once('.').unwrap_or((spec, "0"));
        let action = match action {
            "close" => RowAction::Close,
            "rename" => RowAction::Rename,
            _ => RowAction::NewTab,
        };
        if let Some(workspace) = window.workspaces().get(index.parse::<usize>().unwrap_or(0)) {
            window.row_action(workspace.id, action);
        }
    } else if let Some(path) = step.strip_prefix("workspace:") {
        let path = match path.strip_prefix('~') {
            Some(rest) => term_base::home().join(rest.trim_start_matches('/')),
            None => path.into(),
        };
        window.open_workspace(path);
    } else if let Some(spec) = step.strip_prefix("rename:") {
        let (index, name) = spec.split_once('=').unwrap_or((spec, ""));
        if let Some(workspace) = window.workspaces().get(index.parse::<usize>().unwrap_or(0)) {
            window.set_workspace_name(workspace.id, name);
        }
    } else if let Some(spec) = step.strip_prefix("rowmove:") {
        let (from, to) = spec.split_once('.').unwrap_or((spec, "0"));
        window.drag_row(from.parse().unwrap_or(0), to.parse().unwrap_or(0));
    } else if let Some(spec) = step.strip_prefix("tabmove:") {
        let (from, to) = spec.split_once('.').unwrap_or((spec, "0"));
        if let Some(tab) = window.front_group_tabs().get(from.parse::<usize>().unwrap_or(0)) {
            window.move_tab(*tab, to.parse().unwrap_or(0));
        }
    } else {
        return false;
    }
    true
}

fn print_sidebar(window: &crate::window::Window, name: &str) {
    let home = term_base::home();
    let selected = window.selected_workspace().map(|workspace| workspace.id);
    let front_tab = window.selected_tab_id();
    for workspace in window.workspaces() {
        let row = workspace.entry();
        let mark = if selected == Some(row.id) { "*" } else { " " };
        println!(
            "sidebar {name}: {mark} {} | {} | {:?} | {}",
            row.name,
            row.detail(&home),
            row.mark(),
            row.spoken(&home)
        );
        for tab in workspace.tabs() {
            let entry = tab.entry();
            let mark = if front_tab == Some(tab.id) { "*" } else { " " };
            println!(
                "sidebar {name}:     {mark} {} | {} | {:?} | {:?}",
                entry.heading(&home),
                entry.detail(&home),
                entry.status,
                entry.progress
            );
        }
    }
}
