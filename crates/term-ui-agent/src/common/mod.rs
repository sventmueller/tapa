//! What the agent tab's AppKit parts share beyond `term_ui::kit`, whose alerts, styles, serial numbers and interface
//! size it re-exports here: threads for one piece of work, VoiceOver announcements, views that know when they are in
//! view, and the colours of code tokens.

pub mod notify;
pub mod panel;
pub mod style;
pub mod util;

pub use term_base::diag;
pub use term_render::constant;
pub use term_ui::kit::{alert, registry, ui_size};
