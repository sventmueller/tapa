//! The menu bar as a table: the menus in the order macOS apps have them, App, Shell (Terminal's name for File), Edit,
//! View, Window and Help, with AppKit's standard items and the actions of the key bindings (F167).

use objc2::runtime::Sel;
use objc2::sel;
use splat_i18n::{t, tf};
use term_config::Action;

/// One entry of a menu.
#[derive(Debug)]
pub(super) enum Entry {
    /// An action of the key bindings, with the title the settings and the palette show and the shortcut bound to it.
    Action(Action),
    /// An item AppKit sends along the responder chain, with a fixed shortcut.
    Standard(String, Sel, &'static str),
    /// An item sent along the responder chain whose shortcut the key bindings set, as Copy and Paste.
    Bound(String, Sel, Action),
    /// The item of Sparkle's update check, where the build carries Sparkle.
    Updates,
    /// An item a UI crate builds, given the shortcut the app binds to each of its commands' names.
    Item(MakeItem),
    /// The Services menu, which AppKit fills.
    Services,
    Separator,
    /// A submenu.
    Menu(String, Vec<Entry>),
    /// A hidden item with an action of AppKit's, so that AppKit adds no item of its own for it.
    Hidden(Sel),
}

/// What builds an item of a UI crate.
pub(super) type MakeItem =
    fn(objc2::MainThreadMarker, &dyn Fn(&str) -> String) -> objc2::rc::Retained<objc2_app_kit::NSMenuItem>;

/// The menus of the menu bar, each with its title.
pub(super) fn layout(name: &str) -> Vec<(String, Vec<Entry>)> {
    vec![
        (name.to_string(), app(name)),
        (t("Shell").to_string(), shell()),
        (t("Bearbeiten").to_string(), edit()),
        (t("Darstellung").to_string(), view()),
        (t("Fenster").to_string(), window()),
        (t("Hilfe").to_string(), help(name)),
    ]
}

fn app(name: &str) -> Vec<Entry> {
    use Entry::{Action as A, Separator, Services, Standard, Updates};
    vec![
        Standard(tf("Über {}", &[name]), sel!(termAbout:), ""),
        Updates,
        Separator,
        A(Action::Settings),
        A(Action::ReloadConfig),
        Separator,
        A(Action::SecureKeyboardEntry),
        Separator,
        Services,
        Separator,
        Standard(tf("{} ausblenden", &[name]), sel!(hide:), "cmd+h"),
        Standard(t("Andere ausblenden").into(), sel!(hideOtherApplications:), "alt+cmd+h"),
        Standard(t("Alle einblenden").into(), sel!(unhideAllApplications:), ""),
        Separator,
        Standard(tf("{} beenden", &[name]), sel!(terminate:), "cmd+q"),
    ]
}

fn shell() -> Vec<Entry> {
    use Entry::{Action as A, Bound, Item, Separator};
    vec![
        A(Action::NewWindow),
        A(Action::NewWorkspace),
        A(Action::NewTab),
        A(Action::NewClaudeTab),
        A(Action::NewCodexTab),
        A(Action::NewOpenCodeTab),
        A(Action::NewBrowserTab),
        Separator,
        A(Action::NewFile),
        A(Action::OpenFile),
        Separator,
        Bound(t("Sichern").into(), sel!(termSave:), Action::Save),
        Bound(t("Sichern unter …").into(), sel!(termSaveAs:), Action::SaveAs),
        A(Action::SaveAll),
        Item(|mtm, _| term_ui_editor::open_in_splat_menu_item(mtm)),
        Separator,
        A(Action::RenameWorkspace),
        A(Action::ToggleTrust),
        Separator,
        A(Action::SplitRight),
        A(Action::SplitDown),
        Separator,
        A(Action::SaveOutput),
        Separator,
        A(Action::Close),
        A(Action::ClosePane),
        A(Action::CloseTab),
        A(Action::CloseWorkspace),
        A(Action::CloseWindow),
    ]
}

fn edit() -> Vec<Entry> {
    use Entry::{Action as A, Bound, Item, Menu, Separator, Standard};
    vec![
        Standard(t("Widerrufen").into(), sel!(undo:), "cmd+z"),
        Standard(t("Wiederholen").into(), sel!(redo:), "shift+cmd+z"),
        Separator,
        Standard(t("Ausschneiden").into(), sel!(cut:), "cmd+x"),
        Bound(t("Kopieren").into(), sel!(copy:), Action::Copy),
        A(Action::CopyFormatted),
        Bound(t("Einsetzen").into(), sel!(paste:), Action::Paste),
        Item(term_ui_editor::paste_plain_item),
        Bound(t("Alles auswählen").into(), sel!(selectAll:), Action::SelectAll),
        Separator,
        Bound(t("Leeren").into(), sel!(termClear:), Action::Clear),
        Separator,
        Menu(
            t("Suchen").into(),
            vec![A(Action::Find), A(Action::FindNext), A(Action::FindPrevious)],
        ),
        Separator,
        A(Action::CopyMode),
        A(Action::SelectBlock),
        A(Action::CopyOutput),
        Separator,
        A(Action::ToggleDictation),
    ]
}

fn view() -> Vec<Entry> {
    use Entry::{Action as A, Hidden, Item, Menu, Separator, Standard};
    vec![
        A(Action::CommandPalette),
        Separator,
        A(Action::ToggleSidebar),
        A(Action::ToggleFiles),
        A(Action::ToggleGit),
        A(Action::ToggleInputField),
        Item(term_ui_editor::preview_item),
        Separator,
        Hidden(sel!(toggleTabBar:)),
        Standard(
            t("Alle Tabs einblenden").into(),
            sel!(toggleTabOverview:),
            "shift+cmd+\\",
        ),
        Separator,
        A(Action::FontBigger),
        A(Action::FontSmaller),
        A(Action::FontReset),
        Separator,
        Menu(
            t("Bereiche").into(),
            vec![
                A(Action::ZoomPane),
                A(Action::EqualizePanes),
                Separator,
                A(Action::ResizeLeft),
                A(Action::ResizeRight),
                A(Action::ResizeUp),
                A(Action::ResizeDown),
            ],
        ),
        Menu(
            t("Prompts und Lesezeichen").into(),
            vec![
                A(Action::PreviousPrompt),
                A(Action::NextPrompt),
                A(Action::ToggleBlock),
                Separator,
                A(Action::ToggleBookmark),
                A(Action::PreviousBookmark),
                A(Action::NextBookmark),
            ],
        ),
        Separator,
        A(Action::ScrollTop),
        A(Action::ScrollBottom),
        A(Action::PageUp),
        A(Action::PageDown),
        Separator,
        Standard(
            t("Vollbildmodus aktivieren").into(),
            sel!(toggleFullScreen:),
            "ctrl+cmd+f",
        ),
    ]
}

fn window() -> Vec<Entry> {
    use Entry::{Action as A, Hidden, Menu, Separator, Standard};
    vec![
        Standard(t("Im Dock ablegen").into(), sel!(performMiniaturize:), "cmd+m"),
        Standard(t("Zoomen").into(), sel!(performZoom:), ""),
        Separator,
        A(Action::PreviousWorkspace),
        A(Action::NextWorkspace),
        Separator,
        A(Action::PreviousTab),
        A(Action::NextTab),
        Menu(
            t("Tab auswählen").into(),
            vec![
                A(Action::Tab1),
                A(Action::Tab2),
                A(Action::Tab3),
                A(Action::Tab4),
                A(Action::Tab5),
                A(Action::Tab6),
                A(Action::Tab7),
                A(Action::Tab8),
                A(Action::LastTab),
            ],
        ),
        A(Action::TabSwitcher),
        Separator,
        Standard(t("Tab in neues Fenster bewegen").into(), sel!(moveTabToNewWindow:), ""),
        Standard(t("Alle Fenster zusammenführen").into(), sel!(mergeAllWindows:), ""),
        Separator,
        A(Action::PreviousPane),
        A(Action::NextPane),
        Separator,
        A(Action::QuickTerminal),
        Separator,
        Standard(t("Alle nach vorne bringen").into(), sel!(arrangeInFront:), ""),
        // AppKit would add its own Show Next and Show Previous Tab with ⌃Tab beside the two above.
        Hidden(sel!(selectNextTab:)),
        Hidden(sel!(selectPreviousTab:)),
    ]
}

fn help(name: &str) -> Vec<Entry> {
    use Entry::Standard;
    vec![
        Standard(t("Problem melden …").into(), sel!(termReportProblem:), ""),
        Standard(t("Funktion vorschlagen …").into(), sel!(termSuggestFeature:), ""),
        Standard(tf("{} auf GitHub", &[name]), sel!(termOpenRepository:), ""),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walk<'a>(entries: &'a [Entry], into: &mut Vec<&'a Entry>) {
        for entry in entries {
            into.push(entry);
            if let Entry::Menu(_, inner) = entry {
                walk(inner, into);
            }
        }
    }

    fn all() -> Vec<(String, Vec<Entry>)> {
        layout("Tapa")
    }

    #[test]
    fn the_menus_come_in_the_order_of_macos() {
        let titles: Vec<String> = all().into_iter().map(|(title, _)| title).collect();
        assert_eq!(
            titles,
            ["Tapa", "Shell", "Bearbeiten", "Darstellung", "Fenster", "Hilfe"]
        );
    }

    #[test]
    fn every_action_is_in_the_menu_bar_once() {
        let menus = all();
        let mut entries = Vec::new();
        for (_, items) in &menus {
            walk(items, &mut entries);
        }
        for &action in Action::ALL {
            let count = entries
                .iter()
                .filter(|entry| match entry {
                    Entry::Action(own) | Entry::Bound(_, _, own) => *own == action,
                    _ => false,
                })
                .count();
            assert_eq!(count, 1, "{action:?}");
        }
    }

    #[test]
    fn appkit_adds_no_items_of_its_own_for_the_next_and_previous_tab() {
        let menus = all();
        let (_, window) = menus.iter().find(|(title, _)| title == "Fenster").unwrap();
        for wanted in [sel!(selectNextTab:), sel!(selectPreviousTab:)] {
            assert!(
                window
                    .iter()
                    .any(|entry| matches!(entry, Entry::Hidden(s) if *s == wanted))
            );
        }
    }

    #[test]
    fn the_help_menu_leads_to_the_problem_form_the_ideas_and_the_repository() {
        let menus = all();
        let (_, help) = menus.last().unwrap();
        let actions: Vec<Sel> = help
            .iter()
            .filter_map(|entry| match entry {
                Entry::Standard(_, selector, _) => Some(*selector),
                _ => None,
            })
            .collect();
        assert_eq!(
            actions,
            [
                sel!(termReportProblem:),
                sel!(termSuggestFeature:),
                sel!(termOpenRepository:)
            ]
        );
    }

    #[test]
    fn the_standard_items_have_their_standard_shortcuts() {
        let menus = all();
        let mut entries = Vec::new();
        for (_, items) in &menus {
            walk(items, &mut entries);
        }
        let shortcut = |wanted: Sel| {
            entries.iter().find_map(|entry| match entry {
                Entry::Standard(_, selector, key) if *selector == wanted => Some(*key),
                _ => None,
            })
        };
        assert_eq!(shortcut(sel!(terminate:)), Some("cmd+q"));
        assert_eq!(shortcut(sel!(hide:)), Some("cmd+h"));
        assert_eq!(shortcut(sel!(hideOtherApplications:)), Some("alt+cmd+h"));
        assert_eq!(shortcut(sel!(undo:)), Some("cmd+z"));
        assert_eq!(shortcut(sel!(redo:)), Some("shift+cmd+z"));
        assert_eq!(shortcut(sel!(cut:)), Some("cmd+x"));
        assert_eq!(shortcut(sel!(performMiniaturize:)), Some("cmd+m"));
        assert_eq!(shortcut(sel!(toggleFullScreen:)), Some("ctrl+cmd+f"));
        assert!(shortcut(sel!(arrangeInFront:)).is_some());
        assert_eq!(shortcut(sel!(toggleTabOverview:)), Some("shift+cmd+\\"));
        assert!(shortcut(sel!(mergeAllWindows:)).is_some());
        assert!(shortcut(sel!(moveTabToNewWindow:)).is_some());
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, Entry::Hidden(selector) if *selector == sel!(toggleTabBar:)))
        );
        let app = &menus[0].1;
        assert!(matches!(app.first(), Some(Entry::Standard(_, s, _)) if *s == sel!(termAbout:)));
        assert!(matches!(app.last(), Some(Entry::Standard(_, s, _)) if *s == sel!(terminate:)));
        assert!(app.iter().any(|entry| matches!(entry, Entry::Services)));
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_has_two_in_a_row() {
        fn check(title: &str, items: &[Entry]) {
            let separator = |entry: Option<&Entry>| matches!(entry, Some(Entry::Separator));
            assert!(!separator(items.first()) && !separator(items.last()), "{title}");
            assert!(
                items
                    .windows(2)
                    .all(|pair| !(separator(pair.first()) && separator(pair.get(1)))),
                "{title}"
            );
            for entry in items {
                if let Entry::Menu(inner_title, inner) = entry {
                    check(inner_title, inner);
                }
            }
        }
        for (title, items) in &all() {
            check(title, items);
        }
    }

    #[test]
    fn items_that_ask_for_more_end_in_an_ellipsis() {
        for action in [
            Action::Settings,
            Action::Find,
            Action::SaveOutput,
            Action::CommandPalette,
            Action::TabSwitcher,
            Action::NewWorkspace,
        ] {
            assert!(super::super::title(action).ends_with('…'), "{action:?}");
        }
        assert!(!super::super::title(Action::NewTab).contains('…'));
        assert!(
            !super::super::title(Action::RenameWorkspace).contains('…'),
            "the name turns into a field in its row, without a dialog"
        );
    }
}
