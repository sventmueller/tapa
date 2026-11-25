//! A row of the timeline: the vertical line on the leading side with the entry's dot, and the keyboard selection.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor, NSTableRowView, NSUserInterfaceLayoutDirection};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

use crate::common::util::guarded;

/// Where the line runs, counted from the leading edge.
pub(crate) const LINE_X: f64 = 13.0;
/// How far below the row's top the dot sits: in the middle of the first line of text.
pub(crate) const DOT_Y: f64 = 13.0;
const DOT_RADIUS: f64 = 3.5;

/// Where the dot sits at the text size the user chose, still in the middle of the first line.
fn dot_y() -> f64 {
    let top = crate::timeline::TOP;
    top + (DOT_Y - top) * crate::style::chat(1.0)
}

/// How a row is marked.
#[derive(Clone, Default)]
pub(crate) struct Mark {
    /// The dot's colour; no dot without one.
    pub(crate) dot: Option<Retained<NSColor>>,
    /// Whether the line comes from the row above.
    pub(crate) line_above: bool,
    /// Whether the line goes on to the row below.
    pub(crate) line_below: bool,
}

#[derive(Default)]
pub(crate) struct RowIvars {
    mark: RefCell<Mark>,
    rtl: Cell<bool>,
}

define_class!(
    // SAFETY: NSTableRowView has no subclassing requirements and `TimelineRow` does not implement Drop.
    #[unsafe(super(NSTableRowView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentTimelineRow"]
    #[ivars = RowIvars]
    pub(crate) struct TimelineRow;

    unsafe impl NSObjectProtocol for TimelineRow {}

    impl TimelineRow {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawBackgroundInRect:))]
        fn draw_background(&self, _dirty: NSRect) {
            guarded((), || self.draw_mark());
        }

        #[unsafe(method(drawSelectionInRect:))]
        fn draw_selection(&self, _dirty: NSRect) {
            guarded((), || {
                let bounds = self.bounds();
                let rect = NSRect::new(
                    NSPoint::new(bounds.origin.x + 2.0, bounds.origin.y + 1.0),
                    NSSize::new(bounds.size.width - 4.0, bounds.size.height - 2.0),
                );
                let color = if self.isEmphasized() {
                    NSColor::selectedContentBackgroundColor().colorWithAlphaComponent(0.12)
                } else {
                    NSColor::unemphasizedSelectedContentBackgroundColor().colorWithAlphaComponent(0.5)
                };
                color.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 6.0, 6.0).fill();
            });
        }
    }
);

impl TimelineRow {
    pub(crate) fn new(mtm: MainThreadMarker, mark: Mark) -> Retained<TimelineRow> {
        let this = Self::alloc(mtm).set_ivars(RowIvars::default());
        // SAFETY: NSTableRowView's initializer, called once on the newly allocated object.
        let row: Retained<TimelineRow> = unsafe { msg_send![super(this), init] };
        *row.ivars().mark.borrow_mut() = mark;
        row.ivars()
            .rtl
            .set(row.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft);
        row
    }

    pub(crate) fn set_mark(&self, mark: Mark) {
        *self.ivars().mark.borrow_mut() = mark;
        self.setNeedsDisplay(true);
    }

    fn draw_mark(&self) {
        let mark = self.ivars().mark.borrow();
        let bounds = self.bounds();
        let dot_y = dot_y();
        let x = if self.ivars().rtl.get() {
            bounds.size.width - LINE_X
        } else {
            LINE_X
        };
        if mark.line_above || mark.line_below {
            let top = if mark.line_above { 0.0 } else { dot_y };
            let bottom = if mark.line_below { bounds.size.height } else { dot_y };
            if bottom > top {
                NSColor::separatorColor().setFill();
                NSBezierPath::fillRect(NSRect::new(NSPoint::new(x - 0.5, top), NSSize::new(1.0, bottom - top)));
            }
        }
        if let Some(color) = &mark.dot {
            let rect = NSRect::new(
                NSPoint::new(x - DOT_RADIUS, dot_y - DOT_RADIUS),
                NSSize::new(DOT_RADIUS * 2.0, DOT_RADIUS * 2.0),
            );
            color.setFill();
            NSBezierPath::bezierPathWithOvalInRect(rect).fill();
        }
    }
}
