//! The tab switcher (F57): the command palette's list over every session in every tab of every workspace of every
//! window, found by title, folder, branch, program and workspace; choosing one brings its window, workspace, tab and
//! pane forward.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use objc2_app_kit::NSApplication;
use splat_i18n::{t, tf};
use term_session::Foreground;
use term_ui::palette::Entry;
use term_ui_sidebar::model::short_folder;

use crate::state::app;

/// What the switcher shows of a session.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    view: u64,
    /// The title an agent or program set, if it is worth showing.
    title: Option<String>,
    folder: PathBuf,
    /// Where it is in a remote workspace, shown in place of `folder`.
    place: Option<String>,
    branch: Option<String>,
    /// The program in front, empty where it is not known.
    program: String,
    /// The window's and the tab's place, counted from 1; the window's only where there are several.
    window: Option<usize>,
    /// The workspace's name, where the window has several.
    workspace: Option<String>,
    tab: usize,
}

thread_local! {
    /// The sessions of the open switcher, by the tag of their entry.
    static VIEWS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Opens the switcher over the window in front, or closes it.
pub(crate) fn toggle() {
    let Some(window) = app().front().and_then(|front| front.front_ns_window()) else {
        return;
    };
    let places = places();
    let home = term_base::home();
    let entries = places
        .iter()
        .enumerate()
        .map(|(tag, place)| entry(place, tag, &home))
        .collect();
    VIEWS.with(|views| *views.borrow_mut() = places.iter().map(|place| place.view).collect());
    term_ui::palette::open_with(&window, t("Tab oder Sitzung suchen"), entries, chosen);
}

fn chosen(tag: usize) {
    if let Some(view) = VIEWS.with(|views| views.borrow().get(tag).copied()) {
        bring_forward(view);
    }
}

/// Brings session `view` forward: its window in front, its tab selected, its pane with the keyboard.
pub(crate) fn bring_forward(view: u64) {
    let app = app();
    let Some(window) = app.window_of_view(view) else { return };
    let Some(tab) = window.tabs().into_iter().find(|tab| tab.holds(view)) else {
        return;
    };
    if let Some(terminal) = tab.terminal() {
        terminal.panes.focus(view);
    }
    window.select(tab.id);
    if let Some(ns_window) = window.front_ns_window() {
        ns_window.makeKeyAndOrderFront(None);
    }
    NSApplication::sharedApplication(app.mtm).activate();
}

/// Every session of every window, those of the window in front first.
fn places() -> Vec<Place> {
    let app = app();
    let mut windows = app.windows.borrow().clone();
    let several = windows.len() > 1;
    if let Some(front) = app.front() {
        windows.sort_by_key(|window| window.id() != front.id());
    }
    let mut places = Vec::new();
    for (number, window) in windows.iter().enumerate() {
        let workspaces = window.workspaces();
        for workspace in &workspaces {
            let named = (workspaces.len() > 1).then(|| workspace.name());
            for (index, tab) in workspace.tabs().iter().enumerate() {
                let Some(terminal) = tab.terminal() else { continue };
                let sessions = terminal.sessions.borrow();
                for view in terminal.panes.sessions() {
                    let Some(session) = sessions.get(&view) else { continue };
                    places.push(Place {
                        view,
                        title: session
                            .title
                            .clone()
                            .filter(|title| crate::tab::worth_showing(title, &session.folder)),
                        folder: session.folder.clone(),
                        place: session.place(),
                        branch: session.branch.clone(),
                        program: program(view),
                        window: several.then_some(number + 1),
                        workspace: named.clone(),
                        tab: index + 1,
                    });
                }
            }
        }
    }
    places
}

/// The name of the program in front of session `view`.
fn program(view: u64) -> String {
    let front = term_ui::find(view).and_then(|view| view.with_session(term_session::Session::foreground));
    match front {
        Some(Foreground::Agent(agent)) => format!("{agent:?}").to_lowercase(),
        Some(Foreground::Shell(name) | Foreground::Remote(name) | Foreground::Other(name)) => name,
        Some(Foreground::Unknown) | None => String::new(),
    }
}

/// The palette's entry for `place`: the title or else the folder, and on the right the folder below a title, the
/// program and where it is; the full folder and the branch find it too.
fn entry(place: &Place, tag: usize, home: &Path) -> Entry {
    let folder = place.place.clone().unwrap_or_else(|| short_folder(&place.folder, home));
    let mut right = Vec::new();
    if place.title.is_some() {
        right.push(folder.clone());
    }
    if !place.program.is_empty() {
        right.push(place.program.clone());
    }
    if let Some(workspace) = &place.workspace {
        right.push(workspace.clone());
    }
    right.push(match place.window {
        Some(window) => tf("Fenster {} · Tab {}", &[&window.to_string(), &place.tab.to_string()]),
        None => tf("Tab {}", &[&place.tab.to_string()]),
    });
    let hidden = [
        place
            .place
            .clone()
            .unwrap_or_else(|| place.folder.display().to_string()),
        place.branch.clone().unwrap_or_default(),
        place.program.clone(),
        place.workspace.clone().unwrap_or_default(),
    ]
    .join(" ");
    Entry {
        title: place.title.clone().unwrap_or(folder),
        hidden,
        shortcut: right.join(" · "),
        tag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(view: u64, title: Option<&str>, folder: &str, program: &str) -> Place {
        Place {
            view,
            title: title.map(str::to_string),
            folder: folder.into(),
            place: None,
            branch: Some("main".into()),
            program: program.into(),
            window: None,
            workspace: None,
            tab: usize::try_from(view).unwrap_or(0),
        }
    }

    fn found(query: &str, places: &[Place]) -> Vec<u64> {
        let home = Path::new("/Users/me");
        let entries: Vec<Entry> = places
            .iter()
            .enumerate()
            .map(|(tag, place)| entry(place, tag, home))
            .collect();
        let texts: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.hidden.as_str()))
            .collect();
        term_ui::palette::rank(query, &texts)
            .into_iter()
            .map(|index| places[index].view)
            .collect()
    }

    #[test]
    fn an_entry_shows_title_or_folder_and_where_the_session_is() {
        let home = Path::new("/Users/me");
        let shell = entry(&place(1, None, "/Users/me/Repos/tapa", "zsh"), 0, home);
        assert_eq!(shell.title, "~/Repos/tapa");
        assert_eq!(shell.shortcut, "zsh · Tab 1");
        let mut agent = place(2, Some("Fixing the parser"), "/Users/me/Repos/tapa", "claude");
        agent.window = Some(2);
        agent.workspace = Some("tapa".into());
        let agent = entry(&agent, 1, home);
        assert_eq!(agent.title, "Fixing the parser");
        assert_eq!(agent.shortcut, "~/Repos/tapa · claude · tapa · Fenster 2 · Tab 2");
        assert_eq!(agent.tag, 1);
    }

    #[test]
    fn sessions_are_found_by_title_folder_branch_and_program() {
        let places = [
            place(1, None, "/Users/me/Repos/tapa", "zsh"),
            place(2, Some("Fixing the parser"), "/Users/me/Repos/dango", "claude"),
            place(3, None, "/opt/homebrew/var/log", "nvim"),
        ];
        assert_eq!(found("", &places), [1, 2, 3]);
        assert_eq!(found("parser", &places), [2]);
        assert_eq!(found("dango", &places), [2]);
        assert_eq!(found("homebrew", &places), [3]);
        assert_eq!(found("claude", &places), [2]);
        assert_eq!(found("nvim", &places), [3]);
        assert_eq!(found("tapa", &places)[0], 1);
        assert!(found("main", &places).len() == 3);
    }
}
