//! The browser tabs of every workspace, by the id of the workspace each belongs to: opening, finding and closing them.

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use splat_i18n::t;

use super::address::{self, Address};
use super::host::state;
use super::view::{BrowserView, Loader, TITLE_LIMIT, Wake};
use term_ui::kit::main_soon;
use term_ui::workspace_host::{TabId, TabKind, Tabs, Workspace, WorkspaceId};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

/// The browser tabs of every workspace.
#[derive(Default)]
pub(crate) struct BrowserTabs {
    /// The open browser tabs of all workspaces.
    tabs: RefCell<Vec<Weak<BrowserView>>>,
    /// Numbers the keys of tabs that show no particular address.
    serial: Cell<u64>,
}

fn with_tabs<R>(f: impl FnOnce(&RefCell<Vec<Weak<BrowserView>>>) -> R) -> R {
    f(&state().tabs.tabs)
}

fn with_serial<R>(f: impl FnOnce(&Cell<u64>) -> R) -> R {
    f(&state().tabs.serial)
}

/// Opens a browser tab at `url`, or empty, in `workspace`. A URL whose tab is open selects that tab instead; a tab of a
/// local server reloads, since a dev server that called it has new content. The tab that shows it, unless the
/// workspace closed.
pub fn open_tab(mtm: MainThreadMarker, workspace: WorkspaceId, url: Option<&str>) -> Option<TabId> {
    let engine = super::host::host().search_engine();
    let Some(address) = url.and_then(|url| address::resolve(url, engine)) else {
        let view = BrowserView::new(mtm, workspace, None, unique_key());
        let tab = attach(&view, t("Browser"))?;
        view.start_empty();
        return Some(tab);
    };
    let key = address.key();
    let open = tabs_of(workspace)
        .into_iter()
        .find(|view| view.key() == key || view.current_url().is_some_and(|url| address::key_of(&url) == key));
    if let Some(view) = open {
        Workspace::new(workspace).select_tab(view.key());
        view.revisit(address);
        return view.tab();
    }
    let view = BrowserView::new(mtm, workspace, None, key);
    let tab = attach(&view, t("Browser"))?;
    view.go(address);
    Some(tab)
}

/// Opens a browser tab at `url` in `workspace` that builds its page and loads it only once it first shows, as a tab the
/// workspace restores behind others; its tab, unless the workspace closed.
pub fn open_restored_tab(mtm: MainThreadMarker, workspace: WorkspaceId, url: &str) -> Option<TabId> {
    let engine = super::host::host().search_engine();
    let Some(address) = address::resolve(url, engine) else {
        return open_tab(mtm, workspace, Some(url));
    };
    let title = address::title_before_load(&address, TITLE_LIMIT).unwrap_or_else(|| t("Browser").to_string());
    let view = BrowserView::waiting(mtm, workspace, address.key(), address.clone(), Wake::Go(address));
    attach(&view, &title)
}

/// Opens a browser tab titled `title` in `workspace` that keeps `url` as its address until it first shows; `shown` then
/// gets what loads a page into it, for a page whose server starts only then.
pub fn open_restored_page(
    mtm: MainThreadMarker,
    workspace: WorkspaceId,
    title: &str,
    url: &str,
    shown: impl FnOnce(Loader) + 'static,
) {
    let asked = Address::Url(url.to_string());
    let view = BrowserView::waiting(mtm, workspace, unique_key(), asked, Wake::Run(Box::new(shown)));
    let _ = attach(&view, title);
}

/// Reloads the open tabs of `workspace` that show a page of `origin`, such as `http://localhost:5173`, for a dev server
/// that does not reload by itself after a file was saved (I7).
pub fn reload_matching(workspace: WorkspaceId, origin: &str) {
    let Some(origin) = address::origin(origin) else { return };
    for view in tabs_of(workspace) {
        if view.current_url().and_then(|url| address::origin(&url)).as_deref() == Some(origin.as_str()) {
            view.reload_page();
        }
    }
}

/// The addresses the browser tabs of `workspace` show.
pub fn urls(workspace: WorkspaceId) -> Vec<String> {
    tabs_of(workspace)
        .into_iter()
        .filter_map(|view| view.current_url())
        .collect()
}

/// The address the browser tab `tab` of `workspace` shows, for opening it again.
pub fn url_of_tab(workspace: WorkspaceId, tab: TabId) -> Option<String> {
    tabs_of(workspace)
        .into_iter()
        .find(|view| view.tab() == Some(tab))
        .and_then(|view| view.current_url())
}

/// Opens `url` from a link of `opener`'s page in a new tab beside it.
pub(super) fn open_tab_beside(opener: &BrowserView, url: &str) {
    let view = BrowserView::new(opener.mtm(), opener.workspace_id(), None, unique_key());
    if attach(&view, t("Browser")).is_some() {
        view.go(Address::Url(url.to_string()));
    }
}

/// A tab for a page that `opener`'s page opens with `window.open` or `target=_blank`; WebKit loads it into the web
/// view this returns, which keeps the page's link to its opener.
pub(super) fn open_popup(opener: &BrowserView, configuration: &WKWebViewConfiguration) -> Option<Retained<WKWebView>> {
    let view = BrowserView::new(opener.mtm(), opener.workspace_id(), Some(configuration), unique_key());
    attach(&view, t("Browser")).map(|_| view.web_view())
}

/// Closes the tab of `view` once WebKit is done with the call that asked for it.
pub(super) fn close_later(view: &BrowserView) {
    later(view, |view| {
        if let Some(tab) = view.tab() {
            Workspace::new(view.workspace_id()).close_tab(tab);
        }
    });
}

/// Shows `address` in `view` once WebKit is done with the navigation it cancelled for it.
pub(super) fn go_later(view: &BrowserView, address: Address) {
    later(view, move |view| view.go(address));
}

/// Runs `then` with `view` on the main thread's next turn, while the view is still one of the open tabs.
fn later(view: &BrowserView, then: impl FnOnce(&BrowserView) + Send + 'static) {
    let address = std::ptr::from_ref(view) as usize;
    main_soon(move || {
        let view = with_tabs(|tabs| {
            tabs.borrow()
                .iter()
                .filter_map(Weak::load)
                .find(|view| std::ptr::from_ref::<BrowserView>(view) as usize == address)
        });
        if let Some(view) = view {
            then(&view);
        }
    });
}

/// Adds `view` as a tab titled `title` of its workspace; its id, unless the workspace closed.
fn attach(view: &Retained<BrowserView>, title: &str) -> Option<TabId> {
    let added = Workspace::new(view.workspace_id()).add_tab(
        TabKind::Browser,
        Some(view.key().to_string()),
        title,
        Retained::into_super(view.clone()),
    );
    let id = added?;
    view.set_tab(id);
    with_tabs(|tabs| tabs.borrow_mut().push(Weak::from_retained(view)));
    Some(id)
}

/// The tab whose page is `web`.
pub(super) fn tab_of(web: &WKWebView) -> Option<Retained<BrowserView>> {
    with_tabs(|tabs| {
        tabs.borrow()
            .iter()
            .filter_map(Weak::load)
            .find(|view| view.shows_web(web))
    })
}

/// The browser tab in front in `workspace`.
fn front(workspace: WorkspaceId) -> Option<Retained<BrowserView>> {
    let selected = Workspace::new(workspace).selected_tab()?;
    tabs_of(workspace).into_iter().find(|view| view.tab() == Some(selected))
}

/// Runs a browser command of the menu bar on the browser tab in front of `workspace`, wherever the focus is.
pub fn perform(workspace: WorkspaceId, action: objc2::runtime::Sel) {
    if let Some(view) = front(workspace) {
        view.perform(action);
    }
}

/// Whether the browser command `action` of the menu bar applies to the tab in front of `workspace`.
pub fn validate(workspace: WorkspaceId, action: objc2::runtime::Sel) -> bool {
    front(workspace).is_some_and(|view| view.validate(Some(action)))
}

/// The open browser tabs of `workspace`; forgets closed ones.
fn tabs_of(workspace: WorkspaceId) -> Vec<Retained<BrowserView>> {
    with_tabs(|tabs| {
        let mut tabs = tabs.borrow_mut();
        tabs.retain(|tab| tab.load().is_some());
        tabs.iter()
            .filter_map(Weak::load)
            .filter(|view| view.workspace_id() == workspace)
            .collect()
    })
}

/// A key for a tab that shows no particular address, which no address ever has.
fn unique_key() -> String {
    let serial = with_serial(|serial| {
        serial.set(serial.get() + 1);
        serial.get()
    });
    format!("browser:{serial}")
}
