//! What the self-test reads from and does with the parts of `dialogs.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// A panel with its text fields.
pub(super) type Asking = (Retained<objc2_app_kit::NSAlert>, Vec<Retained<NSTextField>>);

/// The panel with text fields that shows, for the self-test to fill in.
#[derive(Default)]
pub(crate) struct Dialogs {
    asking: RefCell<Option<Asking>>,
}

pub(super) fn with_asking<R>(f: impl FnOnce(&RefCell<Option<Asking>>) -> R) -> R {
    f(&super::super::host::state().dialogs.asking)
}

/// Fills the fields of the panel that asks with `texts` and closes it with its first button, as the self-test's
/// user; whether a panel was asking.
pub fn answer_asking(texts: &[&str]) -> bool {
    let Some((alert, fields)) = with_asking(|asking| asking.borrow().clone()) else {
        return false;
    };
    for (field, text) in fields.iter().zip(texts) {
        field.setStringValue(&ns(text));
    }
    let Some(first) = alert.buttons().firstObject() else {
        return false;
    };
    // SAFETY: Clicking the alert's own button ends its sheet as a person would; a nil sender is allowed.
    unsafe { first.performClick(None) };
    true
}
