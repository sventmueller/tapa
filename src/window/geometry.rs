//! How big a new window is: the settings' columns and rows in cells of the settings' font, and the sidebar; and
//! where a window fits on the screens.

use objc2_app_kit::NSWindow;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use term_config::Config;
use term_render::render::{Fonts, INSET_X, INSET_Y};
use term_render::shape::Typography;

/// How wide the sidebar opens.
pub(crate) const SIDEBAR_WIDTH: f64 = 230.0;
/// The narrowest the sidebar gets.
pub(crate) const SIDEBAR_MIN: f64 = 170.0;
/// The widest the sidebar gets.
pub(crate) const SIDEBAR_MAX: f64 = 360.0;

/// The content size of a new window for `config`, with room for the sidebar when it shows.
pub(crate) fn content_rect(config: &Config, sidebar: bool) -> NSRect {
    let font = config.face();
    let fonts = Fonts::styled(&font.family, font.size, 2.0, &Typography::of(&font));
    let m = fonts.metrics;
    let width = f64::from(config.window.columns) * m.cell_width + 2.0 * INSET_X;
    let height = f64::from(config.window.rows) * m.cell_height + 2.0 * INSET_Y;
    let extra = if sidebar { SIDEBAR_WIDTH + 1.0 } else { 0.0 };
    NSRect::new(
        NSPoint::new(200.0, 200.0),
        NSSize::new(width.ceil() + extra, height.ceil()),
    )
}

/// The smallest content a window keeps: a pane of a usable size, beside the sidebar when it shows.
pub(crate) fn min_content(sidebar: bool) -> NSSize {
    let extra = if sidebar { SIDEBAR_MIN + 1.0 } else { 0.0 };
    NSSize::new(320.0 + extra, 200.0)
}

/// Grows `window` by what its title bar and toolbar cover of the content, so that `size` stays free for the panes.
pub(crate) fn fit_toolbar(window: &NSWindow, size: NSSize) {
    let Some(content) = window.contentView() else { return };
    let covered = content.frame().size.height - window.contentLayoutRect().size.height;
    if covered > 0.0 {
        window.setContentSize(NSSize::new(size.width, size.height + covered));
    }
}

/// Whether most of `frame` shows on one of the screens, so a window from a screen now gone does not open off screen.
pub(super) fn on_a_screen(mtm: objc2::MainThreadMarker, frame: NSRect) -> bool {
    objc2_app_kit::NSScreen::screens(mtm).iter().any(|screen| {
        let visible = screen.visibleFrame();
        let left = frame.origin.x.max(visible.origin.x);
        let right = (frame.origin.x + frame.size.width).min(visible.origin.x + visible.size.width);
        let bottom = frame.origin.y.max(visible.origin.y);
        let top = (frame.origin.y + frame.size.height).min(visible.origin.y + visible.size.height);
        (right - left).max(0.0) * (top - bottom).max(0.0) >= 0.5 * frame.size.width * frame.size.height
    })
}

/// Shrinks `window` to the visible part of its screen where it is bigger, as a size kept from a larger screen may be.
pub(super) fn fit_screen(window: &NSWindow) {
    let Some(screen) = window
        .screen()
        .or_else(|| objc2_app_kit::NSScreen::mainScreen(objc2::MainThreadOnly::mtm(window)))
    else {
        return;
    };
    let visible = screen.visibleFrame();
    let mut frame = window.frame();
    if frame.size.width <= visible.size.width && frame.size.height <= visible.size.height {
        return;
    }
    frame.size.width = frame.size.width.min(visible.size.width);
    frame.size.height = frame.size.height.min(visible.size.height);
    frame.origin.x = frame.origin.x.clamp(
        visible.origin.x,
        visible.origin.x + visible.size.width - frame.size.width,
    );
    frame.origin.y = frame.origin.y.clamp(
        visible.origin.y,
        visible.origin.y + visible.size.height - frame.size.height,
    );
    window.setFrame_display(frame, false);
}
