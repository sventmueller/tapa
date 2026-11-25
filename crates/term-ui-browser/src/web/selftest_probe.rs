//! What the self-test reads from and does with the parts of `web.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// Whether pages open popups without a click, which the self-test's clicks into a window behind others do not
/// always give them.
pub(super) static POPUPS_UNASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lets the pages of tabs opened from now on open popups without a click.
pub fn open_popups_unasked() {
    POPUPS_UNASKED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `web` offers the Web Inspector, from Safari's Develop menu and its own context menu.
pub(crate) fn has_inspector(web: &WKWebView) -> bool {
    // SAFETY: Plain properties; reading the key `enable_inspect_element` set raises no exception once setting it did
    // not.
    unsafe {
        let preferences = web.configuration().preferences();
        let extras: Option<Retained<AnyObject>> =
            objc2::msg_send![&*preferences, valueForKey: &*ns("developerExtrasEnabled")];
        let extras = extras.is_some_and(|value| value.downcast_ref::<NSNumber>().is_some_and(|n| n.boolValue()));
        web.isInspectable() && extras
    }
}
