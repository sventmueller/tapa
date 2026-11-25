//! The self-test's step `editorwheel:<points>`: a trackpad swipe over the editor in front, as AppKit gets one from the
//! trackpad: scroll events with the phases of the gesture, then momentum that slows down; `points` towards the end of
//! the text, or towards its beginning when negative, where a swipe past the first line pulls the text down and lets it
//! spring back. The events go into the app's own queue, not the system's, so the step works behind other apps.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEvent, NSScrollView};
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{CGEvent, CGEventField, CGScrollEventUnit};
use objc2_foundation::NSPoint;

/// The phases of a scroll event's gesture, as `kCGScrollWheelEventScrollPhase` has them.
const BEGAN: i64 = 1;
const CHANGED: i64 = 2;
const ENDED: i64 = 4;
/// The phases of its momentum, as `kCGScrollWheelEventMomentumPhase` has them.
const MOMENTUM_BEGIN: i64 = 1;
const MOMENTUM_CONTINUE: i64 = 2;
const MOMENTUM_END: i64 = 3;

/// The points a finger moves per event.
const STEP: f64 = 40.0;
/// How much of its speed the momentum keeps from one event to the next.
const DECAY: f64 = 0.85;

/// Runs `step` if it is `editorwheel:`; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some(points) = step.strip_prefix("editorwheel:") else {
        return false;
    };
    let Ok(points) = points.parse::<f64>() else {
        println!("editorwheel {points}: not a number");
        return true;
    };
    let Some(scroll) = super::files::front_editor_scroll() else {
        println!("editorwheel {points}: no editor in front");
        return true;
    };
    let Some(at) = place(&scroll) else {
        println!("editorwheel {points}: the editor has no window");
        return true;
    };
    let sign = -points.signum();
    let mut events = vec![(0.0, BEGAN, 0)];
    let mut left = points.abs();
    while left > 0.0 {
        let delta = left.min(STEP);
        events.push((sign * delta, CHANGED, 0));
        left -= delta;
    }
    events.push((0.0, ENDED, 0));
    let mut speed = STEP;
    events.push((sign * speed, 0, MOMENTUM_BEGIN));
    while speed > 1.0 {
        speed *= DECAY;
        events.push((sign * speed, 0, MOMENTUM_CONTINUE));
    }
    events.push((0.0, 0, MOMENTUM_END));
    let shared = NSApplication::sharedApplication(scroll.mtm());
    for (delta, phase, momentum) in events {
        if let Some(event) = event(&at, delta, phase, momentum) {
            shared.postEvent_atStart(&event, false);
        }
    }
    println!("editorwheel {points}: queued");
    true
}

/// The field of a Quartz event that names the window it goes to, which `NSEvent` reads as its window number.
const WINDOW_FIELD: CGEventField = CGEventField(51);

unsafe extern "C" {
    /// Where in its window an event happens, from the window's top left; CoreGraphics exports it without a header.
    fn CGEventSetWindowLocation(event: &CGEvent, point: CGPoint);
}

/// Where the events happen: the middle of the scroll view, from its window's top left, and the window's number.
struct Place {
    point: CGPoint,
    window: i64,
}

fn place(scroll: &NSScrollView) -> Option<Place> {
    let window = scroll.window()?;
    let bounds = scroll.bounds();
    let middle = NSPoint::new(
        bounds.origin.x + bounds.size.width / 2.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
    let in_window = scroll.convertPoint_toView(middle, None);
    Some(Place {
        point: CGPoint::new(in_window.x, window.frame().size.height - in_window.y),
        window: i64::try_from(window.windowNumber()).ok()?,
    })
}

/// A scroll event at `at` by `delta` points, down the text for a negative one, in `phase` of the gesture or
/// `momentum`.
#[expect(clippy::cast_possible_truncation, reason = "a few hundred points")]
fn event(at: &Place, delta: f64, phase: i64, momentum: i64) -> Option<Retained<NSEvent>> {
    let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 1, delta.round() as i32, 0, 0)?;
    let event = Some(&*event);
    CGEvent::set_integer_value_field(event, WINDOW_FIELD, at.window);
    if let Some(event) = event {
        // SAFETY: CoreGraphics' function takes an event and a point and keeps neither.
        unsafe { CGEventSetWindowLocation(event, at.point) };
    }
    CGEvent::set_integer_value_field(event, CGEventField::ScrollWheelEventIsContinuous, 1);
    CGEvent::set_double_value_field(event, CGEventField::ScrollWheelEventPointDeltaAxis1, delta);
    CGEvent::set_double_value_field(event, CGEventField::ScrollWheelEventFixedPtDeltaAxis1, delta);
    CGEvent::set_integer_value_field(event, CGEventField::ScrollWheelEventScrollPhase, phase);
    CGEvent::set_integer_value_field(event, CGEventField::ScrollWheelEventMomentumPhase, momentum);
    event.and_then(NSEvent::eventWithCGEvent)
}
