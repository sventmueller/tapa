//! The app as the browser sees it beside the workspaces its tabs belong to, which it reaches through
//! `term_ui::workspace_host`: the state of every tab, download and accepted certificate, which the app holds, and the
//! settings the tabs follow.

use std::rc::Rc;
use std::sync::OnceLock;

/// What the browser asks of the app beside its workspaces.
pub trait Host {
    /// The browser's state in the running app.
    fn state(&self) -> Rc<State>;
    /// The engine that gets words typed into the address field.
    fn search_engine(&self) -> term_config::SearchEngine;
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the browser tabs belong to; once, before the first tab opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

pub(super) fn host() -> &'static (dyn Host + Sync) {
    *HOST
        .get()
        .expect("the app installs its host before the first browser tab")
}

pub(super) fn state() -> Rc<State> {
    host().state()
}

/// The browser tabs of every workspace, the downloads that run and the certificates of development servers.
#[derive(Default)]
pub struct State {
    pub(super) tabs: super::tabs::BrowserTabs,
    pub(super) downloads: super::downloads::Downloading,
    pub(super) certificates: super::trust::Certificates,
    #[cfg(feature = "selftest")]
    pub(super) dialogs: super::dialogs::Dialogs,
}
