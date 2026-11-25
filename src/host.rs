//! What the crates ask of the app (E3), answered from its state.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use term_base::Spawner;
use term_config::Config;
use term_config::theme::Themes;
use term_ui::kit::WindowId;
use term_ui::{Change, Opened};

use crate::state::{app, try_app};

/// The app as the session views' host.
pub(crate) struct AppHost;

impl term_ui::Host for AppHost {
    fn state(&self) -> Rc<term_ui::State> {
        app().views.clone()
    }

    fn try_state(&self) -> Option<Rc<term_ui::State>> {
        try_app().map(|app| app.views.clone())
    }

    fn opened(&self, window: WindowId, what: Opened) {
        match what {
            Opened::Url(url) => crate::open::url(&url),
            Opened::File { path, .. } => crate::open::file(&path),
            Opened::TerminalHere(folder) => {
                if let Some(window) = app().window(window.get()) {
                    window.new_tab_in(Some(folder));
                }
            }
            Opened::ClosePane(view) => {
                if let Some(window) = app().window(window.get()) {
                    window.close_view(view);
                }
            }
        }
    }

    fn spawner(&self) -> Arc<dyn Spawner> {
        app().processes.clone()
    }

    fn config(&self) -> Rc<Config> {
        app().config()
    }

    fn themes(&self) -> Rc<Themes> {
        app().themes.borrow().clone()
    }

    fn switch_off(&self, path: &[&str]) {
        let written = crate::settings::apply(|doc| {
            doc.set(path, false);
            Ok(true)
        });
        if let Err(error) = written {
            term_base::diag::report("switching a setting off", error);
        }
    }

    fn suggest(&self, view: u64, prefix: &str) -> Option<String> {
        let app = try_app()?;
        if !app.config().input.suggestions {
            return None;
        }
        let context = app.window_of_view(view)?.context(view)?;
        let history = app.history.borrow();
        if prefix.is_empty() {
            history.next_command(&context)
        } else {
            history.suggest(prefix, &context)
        }
    }

    fn context(&self, view: u64) -> Option<term_suggest::Context> {
        let app = try_app()?;
        if !app.config().input.suggestions {
            return None;
        }
        app.window_of_view(view)?.context(view)
    }

    fn changed(&self, view: u64, change: Change) {
        let Some(app) = try_app() else { return };
        if let Some(window) = app.window_of_view(view) {
            window.session_changed(view, change);
        }
    }
}

/// The folder new sessions start in when nothing else decides: the user's home.
pub(crate) fn home() -> PathBuf {
    term_base::home()
}
