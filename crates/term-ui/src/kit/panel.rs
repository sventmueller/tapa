//! The parts every panel of a workspace's sidebar entry shares: its title buttons, its page and source lists, lists
//! that line up with its title, and announcements for VoiceOver.

mod cells;
pub mod chevron;
mod held;
mod items;
mod list;
mod outline;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSAccessibilityPostNotificationWithUserInfo, NSApplication, NSButton, NSColor, NSOutlineView, NSStackView,
    NSUserInterfaceLayoutDirection, NSUserInterfaceLayoutOrientation,
};
use objc2_foundation::{NSDictionary, NSNumber, NSPoint, NSRect, NSSize};

use crate::kit::{ns, set_label};
pub use cells::{KeptCells, reload};
pub use held::{HeldReload, out_of_view, watch_view, when_first_shown};
pub use items::{RowItem, RowItems};
pub use list::*;
pub use outline::{Nesting, PanelOutline};

/// How far a title's buttons keep from the sidebar's trailing edge, so that the last symbol ends where the panel's
/// content does.
pub const ACCESSORY_INSET: f64 = 4.0;
/// The size of the symbols of every panel's title buttons.
pub(crate) const HEADER_POINTS: f64 = 13.0;
/// The side of a title button's square, which makes symbols of any width sit evenly and easy to hit.
pub(crate) const HEADER_BUTTON: f64 = 22.0;
/// The room between a panel's title buttons.
pub(crate) const HEADER_SPACING: f64 = 0.0;
/// `NSAccessibilityPriorityHigh`, which interrupts what VoiceOver is reading.
const HIGH_PRIORITY: i64 = 90;

/// A row for a panel's title buttons, spaced as every panel has them.
pub fn header_row(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    crate::kit::ui_size::follow_spacing(&row, HEADER_SPACING);
    row
}

/// A title button of a panel: the SF Symbol `name` at the size every panel's has, named `title` for VoiceOver and
/// its tooltip, sending `action` to `target`.
pub fn header_button(
    mtm: MainThreadMarker,
    name: &str,
    title: &str,
    target: Option<&AnyObject>,
    action: Sel,
) -> Retained<NSButton> {
    let button = crate::kit::ui_size::symbol_button(mtm, name, title, HEADER_POINTS, target, action);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    set_label(&button, Some(title));
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    for side in [button.widthAnchor(), button.heightAnchor()] {
        let side = side.constraintEqualToConstant(HEADER_BUTTON);
        crate::kit::ui_size::follow_length(&side, HEADER_BUTTON);
        side.setActive(true);
    }
    button
}

/// `frame` of a list's disclosure triangle, or of its first column's cell when `cell`, moved `by` toward the leading
/// edge; the cell keeps its far edge. For lists that keep AppKit's triangle but line up with the panel's title.
pub fn outdent(view: &NSOutlineView, frame: NSRect, by: f64, cell: bool) -> NSRect {
    if frame.size.width <= 0.0 {
        return frame;
    }
    let rtl = view.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
    let x = match (rtl, cell) {
        (false, _) => (frame.origin.x - by).max(0.0),
        (true, false) => frame.origin.x + by,
        (true, true) => frame.origin.x,
    };
    let width = if cell { frame.size.width + by } else { frame.size.width };
    NSRect::new(NSPoint::new(x, frame.origin.y), NSSize::new(width, frame.size.height))
}

/// Whether `window` is on screen and the tab in front of its tab group; the tabs behind count as visible to AppKit.
pub fn in_front(window: &objc2_app_kit::NSWindow) -> bool {
    window.isVisible()
        && window
            .tabGroup()
            .and_then(|group| group.selectedWindow())
            .is_none_or(|selected| std::ptr::eq(&*selected, window))
}

/// `path` with the home folder written as `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let home = term_base::home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Has VoiceOver read `text` out, interrupting what it reads.
pub fn announce(mtm: MainThreadMarker, text: &str) {
    let app = NSApplication::sharedApplication(mtm);
    let message = ns(text);
    let priority = NSNumber::new_i64(HIGH_PRIORITY);
    let info = NSDictionary::<objc2_app_kit::NSAccessibilityNotificationUserInfoKey, AnyObject>::from_retained_objects(
        &[
            term_render::constant!(objc2_app_kit::NSAccessibilityAnnouncementKey),
            term_render::constant!(objc2_app_kit::NSAccessibilityPriorityKey),
        ],
        &[message.into(), priority.into()],
    );
    let notification = term_render::constant!(objc2_app_kit::NSAccessibilityAnnouncementRequestedNotification);
    // SAFETY: The application is an accessibility element, and the dictionary holds the announcement as a string and
    // its priority as a number, as the notification expects.
    unsafe { NSAccessibilityPostNotificationWithUserInfo(&app, notification, Some(&info)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_in_the_home_folder_start_with_a_tilde() {
        let home = term_base::home();
        assert_eq!(display_path(&home.join("Repos/x")), "~/Repos/x");
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(std::path::Path::new("/opt/x")), "/opt/x");
    }
}
