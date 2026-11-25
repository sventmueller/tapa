//! Dictation through Lowspeak in the agent composer and the editor, in builds with the feature `dictation`: the
//! crate's host, the settings it reads and the menu action; without the feature the action is never available.

use objc2::MainThreadMarker;

#[cfg(feature = "dictation")]
mod on {
    use std::rc::Rc;

    use term_config::Action;

    use crate::state::{app, try_app};

    thread_local! {
        static STATE: Rc<term_ui_dictation::State> = Rc::new(term_ui_dictation::State::default());
    }

    pub(super) struct DictationHost;

    impl term_ui_dictation::Host for DictationHost {
        fn state(&self) -> Rc<term_ui_dictation::State> {
            STATE.with(Rc::clone)
        }

        fn shortcut(&self) -> String {
            let Some(app) = try_app() else { return String::new() };
            let (bindings, _) = app.config().bindings();
            bindings
                .iter()
                .find(|binding| binding.action == Action::ToggleDictation)
                .map(|binding| term_ui_dictation::written_shortcut(&binding.chord.to_string()))
                .unwrap_or_default()
        }

        fn selftest(&self) -> bool {
            cfg!(feature = "selftest") && std::env::var_os("TAPA_SELFTEST").is_some()
        }
    }

    /// Takes the settings in force, which the engine reads at the start of each dictation.
    pub(super) fn config_changed() {
        term_ui_dictation::set_config(&app().config().dictation);
        term_ui_dictation::shortcut_changed();
    }
}

/// Before the menus are built: the crate's host, and leaving out macOS's own dictation item of the Edit menu.
pub(crate) fn install() {
    #[cfg(feature = "dictation")]
    {
        term_ui_dictation::install(&on::DictationHost);
        term_ui_dictation::prepare();
    }
}

/// After launch: the settings, fn, and releasing the local model when the app quits.
pub(crate) fn launched() {
    #[cfg(feature = "dictation")]
    {
        on::config_changed();
        term_ui_dictation::launched();
    }
}

/// The settings file was read again.
pub(crate) fn config_changed() {
    #[cfg(feature = "dictation")]
    on::config_changed();
}

/// The menu action: starts or ends a dictation in the field with the focus.
pub(crate) fn toggle(mtm: MainThreadMarker) {
    #[cfg(feature = "dictation")]
    term_ui_dictation::toggle(mtm);
    #[cfg(not(feature = "dictation"))]
    let _ = mtm;
}

/// Whether the menu action can run now.
pub(crate) fn can_toggle(mtm: MainThreadMarker) -> bool {
    #[cfg(feature = "dictation")]
    return term_ui_dictation::can_toggle(mtm);
    #[cfg(not(feature = "dictation"))]
    {
        let _ = mtm;
        false
    }
}

/// Whether a dictation records, for the title of the menu item; `None` in a build without dictation.
pub(crate) fn recording() -> Option<bool> {
    #[cfg(feature = "dictation")]
    return Some(term_ui_dictation::recording());
    #[cfg(not(feature = "dictation"))]
    None
}

/// Whether the build carries dictation.
pub(crate) const BUILT: bool = cfg!(feature = "dictation");
