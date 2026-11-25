//! What the running app keeps: the settings and themes in force, the register of shells, the open windows, and the
//! file watch that reloads the settings (F160). It lives on the main thread for the life of the app.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use objc2::MainThreadMarker;
use term_config::Config;
use term_config::theme::Themes;

use crate::window::Window;

/// The app's state.
pub(crate) struct App {
    pub(crate) mtm: MainThreadMarker,
    pub(crate) config_path: PathBuf,
    pub(crate) config: RefCell<Rc<Config>>,
    pub(crate) themes: RefCell<Rc<Themes>>,
    /// The app's one register of processes, terminals' shells and agents alike (E21).
    pub(crate) processes: Arc<term_process::Supervisor>,
    pub(crate) views: Rc<term_ui::State>,
    pub(crate) windows: RefCell<Vec<Rc<Window>>>,
    next_id: Cell<u64>,
    watcher: RefCell<Option<term_config::Watcher>>,
    /// What reading the settings file last complained about, shown in the settings window.
    pub(crate) warnings: RefCell<Vec<term_config::Warning>>,
    /// Every command the sessions ran, for suggestions (E8, F88).
    pub(crate) history: RefCell<term_suggest::History>,
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// The app's state; only on the main thread, after `start`.
pub(crate) fn app() -> Rc<App> {
    APP.with(|app| app.get().cloned())
        .expect("the app's state is made at start, on the main thread")
}

/// The app's state, unless it is not made yet or this is not the main thread.
pub(crate) fn try_app() -> Option<Rc<App>> {
    APP.try_with(|app| app.get().cloned()).ok().flatten()
}

/// Reads the settings and themes and makes the app's state.
pub(crate) fn start(mtm: MainThreadMarker) {
    let config_path = std::env::var_os("TAPA_CONFIG").map_or_else(term_config::default_path, PathBuf::from);
    let (config, warnings) = Config::load(&config_path).unwrap_or_else(|error| {
        term_base::diag::report("reading the settings", &error);
        (Config::default(), Vec::new())
    });
    for warning in &warnings {
        term_base::diag::report("settings", &warning.0);
    }
    term_ui::set_option_as_meta(config.terminal.option_as_meta != term_config::OptionAsMeta::None);
    apply_appearance(mtm, config.window.appearance);
    term_ui::kit::ui_size::set(config.window.interface_font_size);
    let app = Rc::new(App {
        mtm,
        config_path,
        config: RefCell::new(Rc::new(config)),
        themes: RefCell::new(Rc::new(load_themes())),
        processes: Arc::new(term_process::Supervisor::global().clone()),
        views: Rc::new(term_ui::State::default()),
        windows: RefCell::new(Vec::new()),
        next_id: Cell::new(1),
        watcher: RefCell::new(None),
        warnings: RefCell::new(warnings),
        history: RefCell::new(term_suggest::History::open(
            &term_base::data_dir().join("history.sqlite"),
        )),
    });
    APP.with(|cell| {
        let _ = cell.set(app);
    });
    watch();
}

/// Draws the whole app light or dark, or as the system has it; the terminals take the theme of that appearance.
fn apply_appearance(mtm: MainThreadMarker, appearance: term_config::Appearance) {
    use objc2_app_kit::{NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    // SAFETY: AppKit sets its exported appearance names when it loads and never changes them.
    let name = unsafe {
        match appearance {
            term_config::Appearance::System => None,
            term_config::Appearance::Light => Some(NSAppearanceNameAqua),
            term_config::Appearance::Dark => Some(NSAppearanceNameDarkAqua),
        }
    };
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let wanted = name.and_then(NSAppearance::appearanceNamed);
    let current = app.appearance().map(|own| own.name());
    if current != wanted.as_ref().map(|own| own.name()) {
        app.setAppearance(wanted.as_deref());
    }
}

/// The built-in themes, those in the app's resources and those in the user's themes folder.
fn load_themes() -> Themes {
    let bundled = objc2_foundation::NSBundle::mainBundle()
        .resourcePath()
        .map(|path| PathBuf::from(path.to_string()).join("themes"));
    let own = term_base::data_dir().join("themes");
    let mut folders: Vec<&std::path::Path> = Vec::new();
    if let Some(bundled) = &bundled {
        folders.push(bundled);
    }
    folders.push(&own);
    Themes::load(&folders)
}

/// Reloads the settings whenever their file changes.
fn watch() {
    let app = app();
    let watched = term_config::watch(app.config_path.clone(), || {
        term_base::main_soon(reload);
    });
    match watched {
        Ok(watcher) => *app.watcher.borrow_mut() = Some(watcher),
        Err(error) => term_base::diag::report("watching the settings", &error),
    }
}

/// Reads the settings file again and has every window and session take up what changed.
pub(crate) fn reload() {
    let Some(app) = try_app() else { return };
    let (config, warnings) = match Config::load(&app.config_path) {
        Ok(read) => read,
        Err(error) => {
            term_base::diag::report("reading the settings", &error);
            return;
        }
    };
    term_ui::set_option_as_meta(config.terminal.option_as_meta != term_config::OptionAsMeta::None);
    apply_appearance(app.mtm, config.window.appearance);
    term_ui::kit::ui_size::set(config.window.interface_font_size);
    let before = app.config();
    *app.config.borrow_mut() = Rc::new(config);
    *app.themes.borrow_mut() = Rc::new(load_themes());
    *app.warnings.borrow_mut() = warnings;
    for view in term_ui::all_views() {
        view.apply_config();
    }
    for window in app.windows.borrow().iter() {
        window.config_changed();
    }
    crate::menu::rebuild(app.mtm);
    crate::quick::install();
    crate::settings::config_changed();
    crate::agent::config_changed();
    crate::dictation::config_changed();
    let now = app.config();
    if now.editor != before.editor {
        term_ui_editor::settings_changed();
        crate::preview_tab::settings_changed();
    }
    if now.files != before.files {
        term_ui_files::show_ignored_changed();
    }
}

impl App {
    /// A new id for a window or tab, never reused while the app runs.
    pub(crate) fn next_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }

    /// The settings in force.
    pub(crate) fn config(&self) -> Rc<Config> {
        self.config.borrow().clone()
    }

    /// The window `id`, while it is open.
    pub(crate) fn window(&self, id: u64) -> Option<Rc<Window>> {
        self.windows.borrow().iter().find(|window| window.id() == id).cloned()
    }

    /// The window in front, where menu commands act.
    pub(crate) fn front(&self) -> Option<Rc<Window>> {
        let app = objc2_app_kit::NSApplication::sharedApplication(self.mtm);
        let key = app.keyWindow().or_else(|| app.mainWindow());
        let windows = self.windows.borrow();
        key.and_then(|key| windows.iter().find(|window| window.owns(&key)).cloned())
            .or_else(|| windows.iter().rev().find(|window| window.is_shown()).cloned())
    }

    /// The window that holds the session view `view`.
    pub(crate) fn window_of_view(&self, view: u64) -> Option<Rc<Window>> {
        self.windows
            .borrow()
            .iter()
            .find(|window| window.holds_view(view))
            .cloned()
    }

    /// Forgets window `id`, which closed.
    pub(crate) fn forget_window(&self, id: u64) {
        self.windows.borrow_mut().retain(|window| window.id() != id);
    }
}
