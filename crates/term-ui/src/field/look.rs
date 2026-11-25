//! What the input field looks like around its text, as Warp draws it: the pane's ground in the terminal's background
//! with a separator line along the top; no bezel and no focus ring, as the separator and the chips mark the field and
//! the caret shows that it has the keyboard.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor, NSView};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

use crate::kit::guarded;

/// The room between the pane's edges and the chips and text.
pub(super) const MARGIN_X: f64 = 10.0;
/// The room between the separator and the chips.
pub(super) const TOP: f64 = 7.0;
/// The room between the chips and the text.
pub(super) const GAP: f64 = 3.0;
/// The room below the text.
pub(super) const BOTTOM: f64 = 5.0;

/// The colour the backdrop draws in.
pub(crate) struct Ivars {
    ground: RefCell<Retained<NSColor>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Backdrop` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermInputFieldBackdrop"]
    #[ivars = Ivars]
    pub(crate) struct Backdrop;

    unsafe impl NSObjectProtocol for Backdrop {}

    impl Backdrop {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }
    }
);

impl Backdrop {
    pub(super) fn new(mtm: MainThreadMarker) -> Retained<Backdrop> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            ground: RefCell::new(NSColor::textBackgroundColor()),
        });
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// Draws on `ground`, the terminal's background.
    pub(super) fn set_ground(&self, ground: &NSColor) {
        *self.ivars().ground.borrow_mut() = ground.into();
        self.setNeedsDisplay(true);
    }

    fn draw(&self) {
        let bounds = self.bounds();
        self.ivars().ground.borrow().setFill();
        NSBezierPath::fillRect(bounds);
        let increase_contrast =
            objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
        let line = if increase_contrast {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::separatorColor()
        };
        line.setFill();
        NSBezierPath::fillRect(NSRect::new(NSPoint::ZERO, NSSize::new(bounds.size.width, 1.0)));
    }
}
