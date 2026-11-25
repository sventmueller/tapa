//! The self-test's hooks of `composer.rs`.

use super::*;

impl Composer {
    /// Through the middle of the text, left to right: whether a click there reaches the text view, which takes the
    /// keyboard focus in a key window; then on the card's margin and below the text: whether a click there puts the
    /// focus into the text. A self-test's window is never key, so the text view's own click is not tried.
    pub(crate) fn clicks(&self) -> Vec<bool> {
        use objc2_app_kit::{NSApplication, NSEventType};

        let Some(window) = self.text.window() else {
            return Vec::new();
        };
        // SAFETY: Asking for the superview only reads; the frame view holds the content view while the window shows.
        let Some(frame) = window.contentView().and_then(|content| unsafe { content.superview() }) else {
            return Vec::new();
        };
        frame.layoutSubtreeIfNeeded();
        let text = self.scroll.bounds();
        let mut seen: Vec<bool> = [0.05, 0.5, 0.95]
            .into_iter()
            .map(|share| {
                let inside = NSPoint::new(text.size.width * share, text.size.height / 2.0);
                let point = self.scroll.convertPoint_toView(inside, None);
                frame.hitTest(point).is_some_and(|view| view.isDescendantOf(&self.text))
            })
            .collect();
        let card = self.card.bounds();
        let margin = self
            .card
            .convertPoint_toView(NSPoint::new(4.0, card.size.height / 2.0), None);
        let below = self
            .scroll
            .convertPoint_toView(NSPoint::new(text.size.width / 2.0, -4.0), None);
        let app = NSApplication::sharedApplication(window.mtm());
        let event = |kind: NSEventType, point: NSPoint| {
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
        for point in [margin, below] {
            window.makeFirstResponder(None);
            if let Some(up) = event(NSEventType::LeftMouseUp, point) {
                app.postEvent_atStart(&up, false);
            }
            if let (Some(down), Some(view)) = (event(NSEventType::LeftMouseDown, point), frame.hitTest(point)) {
                // SAFETY: The view takes the press, which lives for the call.
                let () = unsafe { msg_send![&*view, mouseDown: &*down] };
            }
            seen.push(self.has_focus());
        }
        seen
    }
}
