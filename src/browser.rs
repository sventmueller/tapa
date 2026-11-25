//! The browser tabs in the app (E22): `term-ui-browser` adds them to a workspace through `term_ui::workspace_host`,
//! and the app is that crate's `Host` for the state of its tabs and the search engine of the settings.

use std::cell::OnceCell;
use std::rc::Rc;

use term_ui::workspace_host::{TabId, TabKind, WorkspaceId};

use crate::state::app;
use crate::tab::Tab;
use crate::window::Window;

/// The app as the browser tabs' host.
struct BrowserHost;

static HOST: BrowserHost = BrowserHost;

thread_local! {
    static STATE: OnceCell<Rc<term_ui_browser::State>> = const { OnceCell::new() };
}

impl term_ui_browser::Host for BrowserHost {
    fn state(&self) -> Rc<term_ui_browser::State> {
        STATE.with(|state| state.get_or_init(Rc::default).clone())
    }

    fn search_engine(&self) -> term_config::SearchEngine {
        app().config().browser.search_engine
    }
}

/// Hands the browser tabs their host; once, before the first window opens.
pub(crate) fn install() {
    term_ui_browser::install(&HOST);
}

/// Opens a browser tab in the window's selected workspace at `url`, or one that invites an address.
pub(crate) fn open(window: &Window, url: Option<&str>) {
    let Some(workspace) = window.selected_workspace() else {
        return;
    };
    if window.is_quick() {
        return objc2_app_kit::NSBeep();
    }
    let _ = term_ui_browser::open_tab(app().mtm, WorkspaceId::new(workspace.id), url);
}

/// The address the browser tab `tab` of workspace `workspace` shows, which the restore keeps.
pub(crate) fn kept(workspace: u64, tab: &Tab) -> Option<String> {
    tab.as_hosted().filter(|hosted| hosted.kind() == TabKind::Browser)?;
    term_ui_browser::url_of_tab(WorkspaceId::new(workspace), TabId::new(tab.id))
}

/// Opens the browser tab at `url` again in workspace `workspace`, loading only once it first shows; its tab's id.
pub(crate) fn reopen(workspace: u64, url: &str) -> Option<u64> {
    term_ui_browser::open_restored_tab(app().mtm, WorkspaceId::new(workspace), url).map(TabId::get)
}
