//! What the self-test reads from and does with the parts of `tabs.rs`, kept out of its production code.

use super::*;
#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use term_ui::workspace_host::WorkspaceId;

/// What the self-test reads from a browser tab.
pub struct Seen {
    /// The address of the page the tab shows, if any.
    pub url: Option<String>,
    /// The text of the address field.
    pub address: String,
    /// Whether the address field is being edited.
    pub editing: bool,
    /// Whether the Web Inspector is on.
    pub inspector: bool,
    /// The heading of the native page standing in for the web page, while it shows.
    pub failure: Option<String>,
    /// Whether that page offers to accept the server's certificate.
    pub proceed: bool,
    /// The text of the notice bar, while it shows.
    pub notice: Option<String>,
}

/// Presses "Trotzdem öffnen" on the first tab of `workspace` that offers it; whether one did.
pub fn proceed(workspace: WorkspaceId) -> bool {
    let Some(view) = tabs_of(workspace).into_iter().find(|view| view.seen().proceed) else {
        return false;
    };
    view.proceed();
    true
}

/// Presses the key button of the browser tab in front of `workspace`, as the self-test's user.
pub fn fill_login(workspace: WorkspaceId) {
    if let Some(view) = front(workspace) {
        view.fill_login();
    }
}

/// The browser tab in front of `workspace`, which takes the actions of its buttons.
pub fn front_view(workspace: WorkspaceId) -> Option<Retained<objc2_app_kit::NSView>> {
    front(workspace).map(Retained::into_super)
}

/// The address field of the browser tab in front of `workspace`.
pub fn address_field(workspace: WorkspaceId) -> Option<Retained<objc2_app_kit::NSTextField>> {
    front(workspace).map(|view| view.address_field())
}

/// What the self-test reads from each browser tab of `workspace`.
pub fn inspect(workspace: WorkspaceId) -> Vec<Seen> {
    tabs_of(workspace).iter().map(|view| view.seen()).collect()
}

/// Hands `done` the title, address and text of the browser tab in front of `workspace`, or says there is none.
pub fn read_front_page(workspace: WorkspaceId, done: impl FnOnce(String) + 'static) {
    match front(workspace) {
        Some(view) => view.read_page(done),
        None => done("no browser tab in front".to_string()),
    }
}
