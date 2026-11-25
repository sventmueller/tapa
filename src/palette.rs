//! The command palette's entries (F102): every action of the key bindings with its title and the shortcut the
//! settings bind it to, titled as the menu and the settings title it and run through the same path as the menu.

use term_config::Action;
use term_ui::palette::Entry;

use crate::state::app;

/// Every action but the palette's own, with the first chord bound to it.
fn entries() -> Vec<Entry> {
    let (bindings, _) = app().config().bindings();
    Action::ALL
        .iter()
        .enumerate()
        .filter(|(_, action)| {
            **action != Action::CommandPalette
                && (crate::dictation::BUILT || **action != Action::ToggleDictation)
                && !crate::agent::switched_off(**action)
        })
        .map(|(tag, action)| Entry {
            title: crate::settings::action_title(*action),
            hidden: format!("{} {}", action.name(), action.description()),
            shortcut: bindings
                .iter()
                .find(|binding| binding.action == *action)
                .map(|binding| term_ui::palette::glyphs(&binding.chord))
                .unwrap_or_default(),
            tag,
        })
        .collect()
}

/// Opens the palette over the window in front, or closes it.
pub(crate) fn toggle() {
    let Some(window) = app().front().and_then(|front| front.front_ns_window()) else {
        return;
    };
    term_ui::palette::open(&window, entries(), |tag| {
        if let Some(action) = Action::ALL.get(tag) {
            crate::menu::perform(*action);
        }
    });
}
