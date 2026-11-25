//! Sizes, fonts and colours of the views beside the terminal, such as the agent tab. System colours throughout, as
//! Apple's guidelines ask; an agent's own colour only tells its tabs apart, made by `colour::accent` to keep its
//! contrast in every appearance.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSFont, NSFontWeightMedium, NSFontWeightRegular, NSFontWeightSemibold};
use term_render::constant;

use super::colour::{Appearance, Rgb, dynamic};

pub use super::colour::{accent, appearance_of};
pub use super::space;

/// The size of running text in the timeline.
pub const BODY: f64 = 13.0;
/// The size of code, commands and output.
pub const CODE: f64 = 12.0;
/// The size of secondary lines: timestamps, status, hints.
pub const SMALL: f64 = 11.0;
/// The size of headings of panels, sheets and popovers.
pub const HEADING: f64 = 13.0;
/// The size of titles of rows and small popovers.
pub const LABEL: f64 = 12.0;
/// The size of a page's heading in the browser tab.
pub const PAGE_HEADING: f64 = 15.0;
/// The size of a large search field.
pub const FIELD_LARGE: f64 = 17.0;
/// The size of the code a device login asks to type in.
pub const LOGIN_CODE: f64 = 24.0;
/// The size of a page's title.
pub const DISPLAY: f64 = 28.0;
/// The height of a row in a file list.
pub const ROW_HEIGHT: f64 = 22.0;

/// Point sizes of SF Symbols in buttons.
pub mod symbol {
    /// The close button of a tab.
    pub const TAB_CLOSE: f64 = 9.0;
    /// Buttons inside a row or beside a line of text.
    pub const INLINE: f64 = 11.0;
    /// Buttons in panel headers and the agent tab's header.
    pub const SMALL: f64 = 13.0;
    /// Buttons of the browser tab's bar.
    pub const MEDIUM: f64 = 14.0;
    /// Buttons of a toolbar.
    pub const LARGE: f64 = 16.0;
}

/// The system's small control font, which follows the user's text size.
pub fn small_system() -> Retained<NSFont> {
    NSFont::systemFontOfSize(NSFont::smallSystemFontSize())
}

/// The system font at the size of running text.
pub fn body() -> Retained<NSFont> {
    NSFont::systemFontOfSize(BODY)
}

/// The semibold system font at `size`.
pub fn bold(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, constant!(NSFontWeightSemibold))
}

/// The system font of medium weight at `size`.
pub fn medium(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, constant!(NSFontWeightMedium))
}

/// The system font at the size of secondary lines.
pub fn small() -> Retained<NSFont> {
    NSFont::systemFontOfSize(SMALL)
}

/// The monospaced system font at `size`.
pub fn mono(size: f64) -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(size, constant!(NSFontWeightRegular))
}

/// Claude's terracotta as it is in light and dark.
pub const TERRACOTTA: Rgb = Rgb(0xD9, 0x77, 0x57);

thread_local! {
    static TERRACOTTA_COLOR: RefCell<Option<Retained<NSColor>>> = const { RefCell::new(None) };
}

/// Claude's terracotta, darker with Increase Contrast so text and borders keep their contrast.
pub fn terracotta() -> Retained<NSColor> {
    TERRACOTTA_COLOR.with(|color| {
        color
            .borrow_mut()
            .get_or_insert_with(|| {
                dynamic(|appearance| match appearance {
                    Appearance::LightHighContrast => Rgb(0xA8, 0x4A, 0x2E),
                    Appearance::DarkHighContrast => Rgb(0xF0, 0x9A, 0x7E),
                    Appearance::Light | Appearance::Dark => TERRACOTTA,
                })
            })
            .clone()
    })
}

/// The background of inline code, code blocks and output boxes.
pub fn code_background() -> Retained<NSColor> {
    NSColor::quaternarySystemFillColor()
}

/// The background of a removed line of a diff.
pub fn removed_background() -> Retained<NSColor> {
    NSColor::systemRedColor().colorWithAlphaComponent(0.16)
}

/// The background of an added line of a diff.
pub fn added_background() -> Retained<NSColor> {
    NSColor::systemGreenColor().colorWithAlphaComponent(0.16)
}
