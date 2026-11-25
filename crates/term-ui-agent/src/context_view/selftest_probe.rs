//! What the self-test reads from and does with the parts of `context_view.rs`, kept out of its production code.

use super::*;
#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use crate::AgentKey;

/// Clicks the gauge of the tab with `key`, as the mouse would; whether it was there to click.
pub fn click(key: AgentKey) -> bool {
    let gauge = with_gauges(|gauges| gauges.borrow().get(&key).and_then(Weak::load));
    match gauge {
        Some(gauge) if !gauge.isHidden() => {
            // SAFETY: A click without a sender, which sends the button's action to its target as the mouse would.
            unsafe { gauge.performClick(None) };
            true
        }
        _ => false,
    }
}

/// What the self-test reads of the breakdown popover of the tab with `key`.
pub struct Seen {
    /// Whether the gauge offers a breakdown.
    pub offered: bool,
    /// The rows' names and token counts, while the popover is shown.
    pub rows: Vec<String>,
    /// The popover's content, for a picture.
    pub view: Option<Retained<NSView>>,
}

/// What the self-test reads of the breakdown popover of the tab with `key`.
pub fn seen(key: AgentKey) -> Seen {
    let offered = with_gauges(|gauges| {
        gauges
            .borrow()
            .get(&key)
            .and_then(Weak::load)
            .is_some_and(|g| !g.isHidden())
    });
    let view = with_shown(|shown| {
        shown
            .borrow()
            .as_ref()
            .filter(|s| s.key == key && s.popover.isShown())
            .map(|s| s.controller.view())
    });
    let rows = super::super::get(key)
        .and_then(|tab| tab.snapshot().usage().breakdown.clone())
        .filter(|_| view.is_some())
        .map(|b| {
            b.rows
                .iter()
                .map(|r| format!("{} {}", row_name(r), tokens(r.tokens)))
                .collect()
        })
        .unwrap_or_default();
    Seen { offered, rows, view }
}

/// Closes the popover, if one is shown.
pub fn close() {
    if let Some(shown) = with_shown(|shown| shown.borrow_mut().take()) {
        shown.popover.close();
    }
}
