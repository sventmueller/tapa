//! What the terminal's AppKit parts share: building controls, menu items and alerts, Auto Layout shorthands, a guard
//! for a flag, whether a size is being dragged, spacing, the interface's text size, the fonts and colours of the views
//! beside the terminal, the sidebar panels' outline views and row items (`panel`, `file_outline`), serial numbers,
//! and the ids windows are known by.

pub mod alert;
mod appkit;
pub mod colour;
pub mod file_outline;
mod layout;
pub mod live;
mod observe;
pub mod panel;
pub mod registry;
pub mod style;
pub mod ui_size;

pub use appkit::{
    acknowledge, borderless_button, four_colours, hide_from_voiceover, item, label, ns, separator, set_action,
    set_label, set_target, set_title_element, submenu, symbol, symbol_at, symbol_button, tagged, tinted,
};
pub use layout::{activate, auto_layout, center_in, center_y, fixed_size, pin_edges, pin_inset, pin_sides};
pub use observe::{Observation, observe, observe_window_visibility};
pub use term_base::{guarded, main_after, main_soon};

use std::cell::Cell;

/// Raises a flag and restores its previous value on drop, including during unwinding.
#[must_use = "the flag is restored as soon as the guard drops"]
pub struct FlagGuard<'a>(&'a Cell<bool>, bool);

impl<'a> FlagGuard<'a> {
    /// Raises `flag` until the guard drops.
    pub fn raise(flag: &'a Cell<bool>) -> Self {
        Self(flag, flag.replace(true))
    }
}

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}

/// Spacing between controls, in points.
#[expect(missing_docs, reason = "a scale of sizes")]
pub mod space {
    pub const NONE: f64 = 0.0;
    pub const HAIR: f64 = 1.0;
    pub const XXS: f64 = 2.0;
    pub const TIGHT: f64 = 3.0;
    pub const XS: f64 = 4.0;
    pub const SNUG: f64 = 5.0;
    pub const S: f64 = 6.0;
    pub const M: f64 = 8.0;
    pub const L: f64 = 10.0;
    pub const XL: f64 = 12.0;
    pub const XXL: f64 = 14.0;
    pub const SECTION: f64 = 16.0;
    pub const GROUP: f64 = 24.0;
}

/// The number the app gives a window; never reused while the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// The id the app's registry of windows handed out as `number`.
    pub const fn new(number: u64) -> WindowId {
        WindowId(number)
    }

    /// The number under which the app keeps the window.
    pub const fn get(self) -> u64 {
        self.0
    }
}
/// Whether this window can show its contents. AppKit counts the tabs behind the selected tab as visible.
pub fn window_in_front(window: &objc2_app_kit::NSWindow) -> bool {
    window.isVisible()
        && !window.isMiniaturized()
        && window
            .tabGroup()
            .and_then(|group| group.selectedWindow())
            .is_none_or(|selected| std::ptr::eq(&*selected, window))
}
