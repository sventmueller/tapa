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
pub use observe::{Observation, observe};
pub use term_base::{guarded, main_after, main_soon};

use std::cell::Cell;

/// A flag that is up while the guard lives and down once it drops, also when a panic unwinds past it.
#[must_use = "the flag goes down as soon as the guard drops"]
pub struct FlagGuard<'a>(&'a Cell<bool>);

impl<'a> FlagGuard<'a> {
    /// Raises `flag` until the guard drops.
    pub fn raise(flag: &'a Cell<bool>) -> Self {
        flag.set(true);
        Self(flag)
    }
}

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
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
