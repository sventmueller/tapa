//! The sidebar in full screen, as in Dango. AppKit then moves the toolbar into a window of its own that reaches further
//! down than the sidebar's safe area and slides down with the menu bar and the window buttons; the sidebar keeps its
//! rows below it, and the toolbar's background above the sidebar, which outside full screen the sidebar shows through,
//! is cleared so the sidebar runs up to the screen's top.

use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSApplication, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{NSEdgeInsets, NSNotification, NSRect, NSRunLoop};
use term_ui::kit::Observation;

/// How far two edges may lie apart and still count as one, for rounding in AppKit's layout.
const SLACK: f64 = 1.0;

/// Keeps `sidebar` below the toolbar of `window` in full screen and clears the toolbar's background above it, the
/// documents' column `content` telling how far the toolbar reaches; the watches end when the values drop.
pub(super) fn follow(window: &NSWindow, sidebar: &NSView, content: &NSView) -> Vec<Observation> {
    let mtm = MainThreadMarker::from(window);
    let views = Rc::new((Weak::from(window), Weak::from(sidebar), Weak::from(content)));
    let update = {
        let views = views.clone();
        move || {
            if let (Some(window), Some(sidebar), Some(content)) = (views.0.load(), views.1.load(), views.2.load()) {
                fit(&window, &sidebar, &content);
            }
        }
    };
    let later = {
        let update = update.clone();
        move |_: &NSNotification| {
            let update = update.clone();
            let block = RcBlock::new(move || term_ui::kit::guarded((), &update));
            // After AppKit laid the toolbar's window out anew, which it does once the notification is through.
            // SAFETY: Windows and views post on the main thread, whose run loop then calls the block on the same thread.
            unsafe { NSRunLoop::currentRunLoop().performBlock(&block) };
        }
    };
    let window_object: &AnyObject = window;
    // SAFETY: AppKit sets its exported notification names when it loads and never changes them.
    let (entered, exited, key, leaving, moved, resized) = unsafe {
        (
            objc2_app_kit::NSWindowDidEnterFullScreenNotification,
            objc2_app_kit::NSWindowDidExitFullScreenNotification,
            objc2_app_kit::NSWindowDidBecomeKeyNotification,
            objc2_app_kit::NSWindowWillExitFullScreenNotification,
            objc2_app_kit::NSWindowDidMoveNotification,
            objc2_app_kit::NSViewFrameDidChangeNotification,
        )
    };
    let mut watches: Vec<Observation> = [entered, exited, key]
        .into_iter()
        .map(|name| term_ui::kit::observe(mtm, name, Some(window_object), later.clone()))
        .collect();
    for view in [sidebar, content] {
        view.setPostsFrameChangedNotifications(true);
        let object: &AnyObject = view;
        watches.push(term_ui::kit::observe(mtm, resized, Some(object), later.clone()));
    }
    let back = views.clone();
    watches.push(term_ui::kit::observe(mtm, leaving, Some(window_object), move |_| {
        if let (Some(window), Some(sidebar)) = (back.0.load(), back.1.load()) {
            clear_above(&window, &sidebar, false);
        }
    }));
    watches.push(term_ui::kit::observe(mtm, moved, None, move |notification| {
        let toolbar = notification.object();
        let toolbar = toolbar.as_deref().and_then(|object| object.downcast_ref::<NSWindow>());
        if let (Some(toolbar), Some(window)) = (toolbar, views.0.load())
            && is_toolbar_of(toolbar, &window)
        {
            update();
        }
    }));
    watches
}

/// Moves the top of `sidebar`'s safe area below the toolbar of `window` as far as it reaches now, and clears the
/// toolbar's background above it in full screen.
pub(super) fn fit(window: &NSWindow, sidebar: &NSView, content: &NSView) {
    let added = sidebar.additionalSafeAreaInsets();
    let own = sidebar.safeAreaInsets().top - added.top;
    let top = (toolbar_reach(window, sidebar, content) - own).max(0.0);
    if (top - added.top).abs() >= 0.5 {
        sidebar.setAdditionalSafeAreaInsets(NSEdgeInsets {
            top,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
    }
    clear_above(
        window,
        sidebar,
        window.styleMask().contains(NSWindowStyleMask::FullScreen),
    );
}

/// How far the toolbar of `window` in full screen reaches down from the top of `sidebar`, none outside full screen:
/// down to where the safe area of `content` begins, less a tab bar between them, and further by as far as the
/// toolbar's window slid down from the screen's top.
pub(super) fn toolbar_reach(window: &NSWindow, sidebar: &NSView, content: &NSView) -> f64 {
    if !window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return 0.0;
    }
    let top = |view: &NSView, rect: NSRect| {
        let rect = view.convertRect_toView(rect, None);
        rect.origin.y + rect.size.height
    };
    let tab_bar = window
        .tabGroup()
        .filter(|group| group.isTabBarVisible())
        .and_then(|_| {
            let mut frames: Vec<Retained<NSView>> = toolbar_windows(window).into_iter().map(|(_, f)| f).collect();
            frames.extend(frame_view(window));
            frames.iter().find_map(|frame| tab_bar(frame))
        })
        .map_or(0.0, |bar| bar.bounds().size.height);
    let below = top(content, content.safeAreaLayoutGuide().frame()) + tab_bar;
    (top(sidebar, sidebar.bounds()) - below).max(0.0) + toolbar_slide(window)
}

/// How far the toolbar's window of `window` slid down from the window's top, as it does in full screen while the
/// window buttons or the menu bar show; the documents' column keeps its safe area meanwhile.
pub(super) fn toolbar_slide(window: &NSWindow) -> f64 {
    let frame = window.frame();
    toolbar_windows(window).first().map_or(0.0, |(toolbar, _)| {
        let toolbar = toolbar.frame();
        (frame.origin.y + frame.size.height - toolbar.origin.y - toolbar.size.height).max(0.0)
    })
}

/// Clears the toolbar's background above `sidebar` in full screen, or brings it back with `clear` false, before the
/// toolbar returns into `window` on leaving full screen.
fn clear_above(window: &NSWindow, sidebar: &NSView, clear: bool) {
    let column = window.convertRectToScreen(sidebar.convertRect_toView(sidebar.bounds(), None));
    if column.size.width < SLACK {
        return;
    }
    for (toolbar, frame) in toolbar_windows(window) {
        if let Some(background) = background_over(&frame, &toolbar, column) {
            background.setAlphaValue(if clear { 0.0 } else { 1.0 });
        }
    }
}

/// The window AppKit shows `window`'s toolbar in while it is in full screen, with its frame view.
fn toolbar_windows(window: &NSWindow) -> Vec<(Retained<NSWindow>, Retained<NSView>)> {
    let mtm = MainThreadMarker::from(window);
    NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .filter(|toolbar| is_toolbar_of(toolbar, window))
        .filter_map(|toolbar| {
            let frame = frame_view(&toolbar)?;
            Some((toolbar, frame))
        })
        .collect()
}

fn frame_view(window: &NSWindow) -> Option<Retained<NSView>> {
    // SAFETY: Asking for the superview only reads; the frame view holds the content view while it shows.
    window.contentView().and_then(|content| unsafe { content.superview() })
}

fn is_toolbar_of(toolbar: &NSWindow, window: &NSWindow) -> bool {
    // Key-value observing puts a prefix before the class's name.
    toolbar
        .class()
        .name()
        .to_bytes()
        .ends_with(b"NSToolbarFullScreenWindow")
        && same_columns(toolbar.frame(), window.frame())
}

fn tab_bar(view: &NSView) -> Option<Retained<NSView>> {
    if view.class().name().to_bytes().ends_with(b"NSTabBar") && !view.isHiddenOrHasHiddenAncestor() {
        return Some(objc2::Message::retain(view));
    }
    view.subviews().iter().find_map(|child| tab_bar(&child))
}

/// The toolbar's background whose columns on screen are those of `column`.
fn background_over(view: &NSView, toolbar: &NSWindow, column: NSRect) -> Option<Retained<NSView>> {
    if view.class().name().to_bytes().ends_with(b"NSTitlebarBackgroundView") {
        let on_screen = toolbar.convertRectToScreen(view.convertRect_toView(view.bounds(), None));
        return same_columns(on_screen, column).then(|| objc2::Message::retain(view));
    }
    view.subviews()
        .iter()
        .find_map(|child| background_over(&child, toolbar, column))
}

fn same_columns(a: NSRect, b: NSRect) -> bool {
    (a.origin.x - b.origin.x).abs() < SLACK && (a.size.width - b.size.width).abs() < SLACK
}

/// For the self-test: whether `window` is in full screen, how far its toolbar slid and reaches, the sidebar's safe area
/// at its top, and how far the first row of `sidebar` begins below the toolbar's window, negative while under it.
#[cfg(feature = "selftest")]
pub(super) fn state(window: &NSWindow, sidebar: &NSView) -> String {
    let row = first_row(sidebar).map(|(table, rect)| {
        let rect = window.convertRectToScreen(table.convertRect_toView(rect, None));
        rect.origin.y + rect.size.height
    });
    let toolbar = toolbar_windows(window)
        .first()
        .map(|(toolbar, _)| toolbar.frame().origin.y);
    let clearance = row.zip(toolbar).map(|(row, toolbar)| format!("{:.1}", toolbar - row));
    format!(
        "full screen {}, slide {:.1}, safe top {:.1} (added {:.1}), first row below toolbar by {}",
        window.styleMask().contains(NSWindowStyleMask::FullScreen),
        toolbar_slide(window),
        sidebar.safeAreaInsets().top,
        sidebar.additionalSafeAreaInsets().top,
        clearance.unwrap_or_else(|| "-".into()),
    )
}

/// For the self-test: slides the toolbar's window of `window` down by `by` points, as the menu bar does in full screen.
#[cfg(feature = "selftest")]
pub(super) fn slide_toolbar(window: &NSWindow, by: f64) {
    for (toolbar, _) in toolbar_windows(window) {
        let mut frame = toolbar.frame();
        frame.origin.y -= by;
        toolbar.setFrame_display(frame, true);
    }
}

#[cfg(feature = "selftest")]
fn first_row(view: &NSView) -> Option<(Retained<objc2_app_kit::NSTableView>, NSRect)> {
    if let Some(table) = view.downcast_ref::<objc2_app_kit::NSTableView>()
        && table.numberOfRows() > 0
        && !table.isHiddenOrHasHiddenAncestor()
    {
        return Some((objc2::Message::retain(table), table.rectOfRow(0)));
    }
    view.subviews().iter().find_map(|child| first_row(&child))
}
