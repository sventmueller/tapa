//! How a terminal's screen is drawn (E5): the monospaced faces and cells drawn with Core Graphics or as Metal quads
//! over a texture of sprites, line and block characters as shapes, and the colours of both appearances. P1a draws
//! with this renderer copied from Dango; the rebuild on a render thread per display follows in P1b (R18).
//!
//! Increase Contrast gives the dark appearance a black ground:
//!
//! ```
//! use term_session::Rgb;
//! use term_render::palette::theme;
//!
//! assert_eq!(theme(true, true).palette.background, Rgb::new(0, 0, 0));
//! assert_ne!(theme(true, false).palette.background, Rgb::new(0, 0, 0));
//! ```

pub mod boxdraw;
pub mod ghost;
pub mod gpu;
mod lines;
pub mod palette;
pub mod render;
pub mod shape;
pub mod sprites;
pub mod thread;
pub mod tint;

use objc2::rc::Retained;
use objc2_foundation::NSString;

/// Reads a constant that AppKit or Foundation exports.
#[macro_export]
macro_rules! constant {
    ($name:path) => {
        // SAFETY: the frameworks set their exported constants when they load, before any Rust code runs, and never
        // change them.
        unsafe { $name }
    };
}

/// The `NSString` with the text of `s`.
pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}
