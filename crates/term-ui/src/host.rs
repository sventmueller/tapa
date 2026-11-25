//! What the terminal views need of the app (E3): the registry of open views, kept in the app's state, the settings,
//! the spawner for shells, the window a view belongs to, which it names by the window's id and asks to show what it
//! opened, and what a session reports, which the app shows in its tabs and sidebar.

use crate::kit::WindowId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use objc2::rc::{Retained, Weak};
use term_base::{Ending, Spawner};
use term_config::Config;
use term_config::theme::Themes;
use term_session::Event;

use super::view::TerminalView;
use crate::kit::main_soon;

/// Something the user opened from a terminal, which the app shows where it belongs .
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// A web address, for a browser tab.
    Url(String),
    /// A file at a line and column, both counted from 1.
    #[expect(missing_docs, reason = "the variant says it")]
    File { path: PathBuf, line: usize, column: usize },
    /// A new terminal in this folder.
    TerminalHere(PathBuf),
    /// The pane of the session view with this id closes, as the user asked in its context menu.
    ClosePane(u64),
}

/// What a session reported that the app shows outside the view: in its tab, the sidebar or the Dock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The title the program set, or none for the default; at most every `TITLE_GAP`.
    Title(Option<String>),
    /// The shell's working directory on this Mac (OSC 7).
    Directory(PathBuf),
    /// The shell's working directory on another machine, at the other end of ssh (OSC 7).
    Elsewhere(PathBuf),
    /// The shell shows its prompt (OSC 133 A).
    Prompt,
    /// The user's input begins after the prompt (OSC 133 B).
    Input,
    /// A command started (OSC 133 C), with its text where the integration sent it.
    CommandStarted(Option<String>),
    /// The command ended with its exit code (OSC 133 D).
    CommandFinished(Option<i32>),
    /// The program rang the bell while the view was not in front.
    Bell,
    /// A program reported its progress (OSC 9;4), or `None` that it is done.
    Progress(Option<term_session::event::Progress>),
    /// A program asked for a notification (OSC 9, OSC 777).
    Notification(term_session::event::Notification),
    /// The program in the view ended.
    Exited(Ending),
    /// The view took the keyboard focus, as a click into its pane does.
    Focused,
    /// What the input field's switch shows changed (`TerminalView::field_look`).
    Field,
}

/// The app the terminal views live in, as far as they need it.
pub trait Host {
    /// The terminal views of the running app.
    fn state(&self) -> Rc<State>;
    /// The terminal views unless the app has none (yet), for code such as `Drop` that must not fail.
    fn try_state(&self) -> Option<Rc<State>>;
    /// Shows what the user opened from a terminal in window `window`.
    fn opened(&self, window: WindowId, what: Opened);
    /// Keeps the shells the views start.
    fn spawner(&self) -> Arc<dyn Spawner>;
    /// The settings in force.
    fn config(&self) -> Rc<Config>;
    /// The themes the settings can name.
    fn themes(&self) -> Rc<Themes>;
    /// Tells the app what the session of view `view` reported.
    fn changed(&self, view: u64, change: Change);
    /// The command the history suggests after `prefix` in view `view`'s folder, or without a prefix the command most
    /// likely to come next (E8).
    fn suggest(&self, view: u64, prefix: &str) -> Option<String>;
    /// Where the user types in view `view`, for the delayed stage's next command (F210); none while suggestions are
    /// off.
    fn context(&self, view: u64) -> Option<term_suggest::Context> {
        let _ = view;
        None
    }
    /// Switches off the setting at `path` in the settings file, as a warning's "do not ask again" asks.
    fn switch_off(&self, path: &[&str]) {
        let _ = path;
    }
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the terminal views belong to; once, before the first terminal opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

pub(super) fn host() -> &'static (dyn Host + Sync) {
    *HOST
        .get()
        .expect("the app installs its host before the first terminal opens")
}

/// The terminal views of every window by id.
#[derive(Default)]
pub struct State {
    views: RefCell<HashMap<u64, Weak<TerminalView>>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Whether ⌥ sends Meta for every key in all terminals, or only where it types no character; from the settings.
static OPTION_AS_META: AtomicBool = AtomicBool::new(false);

/// Makes ⌥ Meta for every key in all terminals, or only where it types no character.
pub fn set_option_as_meta(on: bool) {
    OPTION_AS_META.store(on, Ordering::Relaxed);
}

pub(super) fn option_as_meta() -> bool {
    OPTION_AS_META.load(Ordering::Relaxed)
}

pub(super) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn register(view: &TerminalView) {
    host().state().views.borrow_mut().insert(view.id(), Weak::from(view));
}

pub(super) fn forget(id: u64) {
    if let Some(state) = HOST.get().and_then(|host| host.try_state()) {
        state.views.borrow_mut().remove(&id);
    }
}

/// The terminal view `id`, while it is open.
pub fn find(id: u64) -> Option<Retained<TerminalView>> {
    host().state().views.borrow().get(&id).and_then(Weak::load)
}

/// The terminals open now, in every window.
pub fn all_views() -> Vec<Retained<TerminalView>> {
    host().state().views.borrow().values().filter_map(Weak::load).collect()
}

/// Hands the session's events, which arrive on its reader thread, to the view on the main thread.
pub(super) fn listener(id: u64) -> impl Fn(Event) + Send + Sync + 'static {
    move |event| {
        main_soon(move || {
            if let Some(view) = find(id) {
                view.handle(event);
            }
        });
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
