//! The self-test's mouse: `drag:<x>/<y>-<x>/<y>` presses the left button at the first point of the terminal in front
//! and lets go at the second, through the window as a real drag does; points are fractions of the terminal's width
//! and height from its top left.

use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;

use crate::state::app;

/// Runs `step` if it is `drag:` or `mouse:`; whether it was.
pub(super) fn run(step: &str) -> bool {
    if step == "windowframe" {
        window_frame();
        return true;
    }
    if let Some(spec) = step.strip_prefix("screen:") {
        screen(spec);
        return true;
    }
    if let Some(spec) = step.strip_prefix("mouse:") {
        single(spec);
        return true;
    }
    if let Some(spec) = step.strip_prefix("rowdrag:") {
        row_drag(spec);
        return true;
    }
    if let Some(title) = step.strip_prefix("contextmenu:") {
        context_menu(title);
        return true;
    }
    let Some(spec) = step.strip_prefix("drag:") else {
        return false;
    };
    let Some((from, to)) = spec
        .split_once('-')
        .and_then(|(a, b)| Some((fraction(a)?, fraction(b)?)))
    else {
        println!("drag {spec}: not <x>/<y>-<x>/<y>");
        return true;
    };
    let Some(view) = app().front().and_then(|window| window.focused_view()) else {
        return true;
    };
    let Some(window) = view.window() else { return true };
    let mtm = objc2::MainThreadMarker::new().expect("the self-test runs on the main thread");
    #[expect(
        deprecated,
        reason = "the self-test must take the mouse even while another app is in front"
    )]
    objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    println!("drag key {}", window.isKeyWindow());
    let bounds = view.bounds();
    let at = |(x, y): (f64, f64)| {
        let down = if view.isFlipped() { y } else { 1.0 - y };
        let local = NSPoint::new(bounds.size.width * x, bounds.size.height * down);
        view.convertPoint_toView(local, None)
    };
    let (start, end) = (at(from), at(to));
    let middle = NSPoint::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0);
    for (kind, point) in [
        (NSEventType::LeftMouseDown, start),
        (NSEventType::LeftMouseDragged, middle),
        (NSEventType::LeftMouseDragged, end),
        (NSEventType::LeftMouseUp, end),
    ] {
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            point,
            NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        );
        if let Some(event) = event {
            window.sendEvent(&event);
        }
    }
    // SAFETY: The content view's superview is the window's frame view, which lives as long as the window.
    let frame_view = window.contentView().and_then(|content| unsafe { content.superview() });
    let hit = frame_view.and_then(|frame| frame.hitTest(frame.convertPoint_fromView(start, None)));
    let class = hit.map_or_else(
        || "none".to_string(),
        |hit| hit.class().name().to_string_lossy().into_owned(),
    );
    let selected = view
        .with_session(term_session::Session::selected_text)
        .flatten()
        .map_or(0, |text| text.len());
    println!("drag {spec}: hit {class}, selected {selected} bytes");
    true
}

fn fraction(text: &str) -> Option<(f64, f64)> {
    let (x, y) = text.split_once('/')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// `mouse:<down|drag|up>:<x>/<y>`: one event of a drag, so steps such as `wait:` can come between them.
fn single(spec: &str) {
    let Some((kind, point)) = spec.split_once(':').and_then(|(kind, at)| Some((kind, fraction(at)?))) else {
        println!("mouse {spec}: not <down|drag|up>:<x>/<y>");
        return;
    };
    let kind = match kind {
        "down" => NSEventType::LeftMouseDown,
        "drag" => NSEventType::LeftMouseDragged,
        _ => NSEventType::LeftMouseUp,
    };
    let Some(view) = app().front().and_then(|window| window.focused_view()) else {
        return;
    };
    let Some(window) = view.window() else { return };
    let bounds = view.bounds();
    let down = if view.isFlipped() { point.1 } else { 1.0 - point.1 };
    let at = view.convertPoint_toView(
        NSPoint::new(bounds.size.width * point.0, bounds.size.height * down),
        None,
    );
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        at,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    );
    if let Some(event) = event {
        window.sendEvent(&event);
    }
}

/// `screen:<x>/<y>-<x>/<y>`: writes the two points of the terminal in front as global display coordinates, top left
/// origin, to `$TAPA_SELFTEST_DIR/screen.txt`, for a tool outside the app that drives the real mouse.
fn screen(spec: &str) {
    let Some((from, to)) = spec
        .split_once('-')
        .and_then(|(a, b)| Some((fraction(a)?, fraction(b)?)))
    else {
        return;
    };
    let Some(view) = app().front().and_then(|window| window.focused_view()) else {
        return;
    };
    let Some(window) = view.window() else { return };
    let Some(height) = window.screen().map(|screen| screen.frame().size.height) else {
        return;
    };
    let bounds = view.bounds();
    let global = |(x, y): (f64, f64)| {
        let down = if view.isFlipped() { y } else { 1.0 - y };
        let local = NSPoint::new(bounds.size.width * x, bounds.size.height * down);
        let at = window.convertPointToScreen(view.convertPoint_toView(local, None));
        (at.x, height - at.y)
    };
    let ((x1, y1), (x2, y2)) = (global(from), global(to));
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let _ = std::fs::write(dir.join("screen.txt"), format!("{x1} {y1} {x2} {y2}"));
    println!("screen {spec}: {x1} {y1} {x2} {y2}");
}

/// `windowframe`: writes the front window's frame as global display coordinates, top left origin, to
/// `$TAPA_SELFTEST_DIR/window.txt`, for a tool outside the app that drives the real mouse.
fn window_frame() {
    let Some(window) = app()
        .front()
        .and_then(|window| window.focused_view())
        .and_then(|view| view.window())
    else {
        return;
    };
    let Some(height) = window.screen().map(|screen| screen.frame().size.height) else {
        return;
    };
    let frame = window.frame();
    let top = height - frame.origin.y - frame.size.height;
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(std::env::temp_dir, std::path::PathBuf::from);
    let _ = std::fs::write(
        dir.join("window.txt"),
        format!("{} {top} {} {}", frame.origin.x, frame.size.width, frame.size.height),
    );
    println!(
        "windowframe: {} {top} {} {}",
        frame.origin.x, frame.size.width, frame.size.height
    );
}

/// The context menu of the terminal in front, as a right click there opens it: its items printed, `title` chosen.
fn context_menu(title: &str) {
    let Some(view) = app().front().and_then(|window| window.focused_view()) else {
        return println!("contextmenu: no terminal");
    };
    let Some(window) = view.window() else { return };
    let at = view.convertPoint_toView(NSPoint::new(10.0, 10.0), None);
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::RightMouseDown,
        at,
        NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    );
    let Some(menu) = event.and_then(|event| view.menuForEvent(&event)) else {
        return println!("contextmenu: none");
    };
    let titles: Vec<String> = menu.itemArray().iter().map(|item| item.title().to_string()).collect();
    println!("contextmenu: {titles:?}");
    if let Some(at) = titles.iter().position(|own| own == title) {
        menu.performActionForItemAtIndex(isize::try_from(at).unwrap_or(0));
    }
}

/// `rowdrag:<from>.<to>`: presses the mouse on sidebar row `from` and lets go on the top edge of row `to`, the events
/// queued in the app for the table's own tracking, as a drag of the row by hand.
fn row_drag(spec: &str) {
    let (from, to) = spec.split_once('.').unwrap_or((spec, "0"));
    let (Ok(from), Ok(to)) = (from.parse::<usize>(), to.parse::<usize>()) else {
        return println!("rowdrag {spec}: not <from>.<to>");
    };
    let Some((sidebar, window)) = app().front().and_then(|front| front.front_sidebar()) else {
        return;
    };
    let (Some(start), Some(end)) = (sidebar.row_point(from, 0.5), sidebar.row_point(to, 0.05)) else {
        return println!("rowdrag {spec}: no such row");
    };
    let mtm = objc2::MainThreadMarker::new().expect("the self-test runs on the main thread");
    let shared = objc2_app_kit::NSApplication::sharedApplication(mtm);
    let event = |kind, point: NSPoint| {
        NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            point,
            NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        )
    };
    for step in 1..=10 {
        let f = f64::from(step) / 10.0;
        let point = NSPoint::new(start.x + (end.x - start.x) * f, start.y + (end.y - start.y) * f);
        if let Some(moved) = event(NSEventType::LeftMouseDragged, point) {
            shared.postEvent_atStart(&moved, false);
        }
    }
    if let Some(up) = event(NSEventType::LeftMouseUp, end) {
        shared.postEvent_atStart(&up, false);
    }
    // The window of an app behind others is not key and would take the press as the click that activates it.
    // SAFETY: The content view's superview is the window's frame view, which lives as long as the window.
    let frame_view = window.contentView().and_then(|content| unsafe { content.superview() });
    if let (Some(down), Some(frame_view)) = (event(NSEventType::LeftMouseDown, start), frame_view)
        && let Some(target) = frame_view.hitTest(start)
    {
        target.mouseDown(&down);
    }
}
