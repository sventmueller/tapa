//! The scrollbar (F106): a native overlay scroller at the view's right edge that shows where in the history the view
//! is, with marks for prompts, search matches and failed commands from the session's `ScrollMarks`.
//!
//! It shows while the view is scrolled back and for a moment after the view moved, as overlay scrollers do; at the
//! live screen it hides, so that it neither covers the last column nor takes its clicks. With "Show scroll bars:
//! Always" in the system settings it stays while there is history.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSControlSize, NSScroller, NSScrollerKnobStyle, NSScrollerPart,
    NSScrollerStyle, NSView,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use term_session::Scroll;
use term_session::session::{ScrollMark, ScrollMarks};

use super::view::TerminalView;
use crate::kit::{guarded, main_after};

/// How long the scrollbar stays after the view moved while it is back at the live screen.
const LINGER: Duration = Duration::from_millis(1500);
/// How often the marks are read again while the scrollbar shows, for prompts and matches that came meanwhile.
const MARKS_EVERY: Duration = Duration::from_millis(500);
/// A mark's height in points.
const MARK_HEIGHT: f64 = 2.0;

/// What the scrollbar keeps.
pub(crate) struct Bar {
    /// The id of its terminal view.
    view: u64,
    /// Each mark's place from the slot's top, 0 to 1, in drawing order.
    marks: RefCell<Vec<(f64, ScrollMark)>>,
    /// The marks are to be read again.
    stale: Cell<bool>,
    read: Cell<Option<Instant>>,
    /// The view's distance from the live screen in lines when the scrollbar last looked.
    offset: Cell<Option<usize>>,
    linger_until: Cell<Option<Instant>>,
    /// A look after `linger_until` is due.
    pending: Cell<bool>,
}

define_class!(
    // SAFETY: NSScroller has no subclassing requirements and `ScrollBar` does not implement Drop.
    /// The terminal view's scrollbar, which draws the marks over the slot and scrolls the session it shows.
    #[unsafe(super(NSScroller))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermTerminalScroller"]
    #[ivars = Bar]
    pub(crate) struct ScrollBar;

    unsafe impl NSObjectProtocol for ScrollBar {}

    impl ScrollBar {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let () = unsafe { msg_send![super(self), drawRect: dirty] };
            guarded((), || self.draw_marks());
        }

        #[unsafe(method(termScrolled:))]
        fn scrolled(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.follow());
        }
    }
);

impl ScrollBar {
    fn new(mtm: MainThreadMarker, view: u64, frame: NSRect) -> Retained<ScrollBar> {
        let this = Self::alloc(mtm).set_ivars(Bar {
            view,
            marks: RefCell::new(vec![]),
            stale: Cell::new(true),
            read: Cell::new(None),
            offset: Cell::new(None),
            linger_until: Cell::new(None),
            pending: Cell::new(false),
        });
        // SAFETY: NSScroller's designated initializer, called once on the newly allocated object.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setScrollerStyle(NSScrollerStyle::Overlay);
        this.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMinXMargin | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        this.setEnabled(true);
        this.setHidden(true);
        let target: &AnyObject = &this;
        // SAFETY: A control holds its target weakly; the target is the scroller itself, whose class has the action.
        unsafe {
            this.setTarget(Some(target));
            this.setAction(Some(sel!(termScrolled:)));
        }
        this
    }

    /// Scrolls the session to where the knob was dragged, or a page for a click beside it.
    fn follow(&self) {
        let Some(view) = crate::find(self.ivars().view) else {
            return;
        };
        let part = self.hitPart();
        let value = self.doubleValue();
        view.with_session(|session| {
            if part == NSScrollerPart::DecrementPage {
                return session.scroll(Scroll::PageUp);
            }
            if part == NSScrollerPart::IncrementPage {
                return session.scroll(Scroll::PageDown);
            }
            let rows = session.rows_view();
            let history = rows.rows.saturating_sub(rows.lines);
            let target = top_for(value, history);
            let lines = i64::try_from(rows.top).unwrap_or(0) - i64::try_from(target).unwrap_or(0);
            if lines != 0 {
                session.scroll(Scroll::Lines(i32::try_from(lines).unwrap_or(0)));
            }
        });
        view.refresh_now();
    }

    fn draw_marks(&self) {
        let marks = self.ivars().marks.borrow();
        if marks.is_empty() {
            return;
        }
        let slot = self.rectForPart(NSScrollerPart::KnobSlot);
        let slot = if slot.size.height > 0.0 { slot } else { self.bounds() };
        let flipped = self.isFlipped();
        let inset = (slot.size.width * 0.2).round();
        let mut colour = None;
        for &(fraction, mark) in marks.iter() {
            if colour != Some(mark) {
                colour = Some(mark);
                colour_of(mark).setFill();
            }
            let along = fraction * (slot.size.height - MARK_HEIGHT);
            let y = if flipped {
                slot.origin.y + along
            } else {
                slot.origin.y + slot.size.height - MARK_HEIGHT - along
            };
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(slot.origin.x + inset, y.round()),
                NSSize::new((slot.size.width - 2.0 * inset).max(1.0), MARK_HEIGHT),
            ));
        }
    }
}

fn colour_of(mark: ScrollMark) -> Retained<NSColor> {
    match mark {
        ScrollMark::Prompt => NSColor::secondaryLabelColor(),
        ScrollMark::Hit => NSColor::systemYellowColor(),
        ScrollMark::Failed => NSColor::systemRedColor(),
    }
}

/// The row at the view's top for a knob at `value`, 0 at the oldest row and 1 at the live screen.
fn top_for(value: f64, history: usize) -> usize {
    let top = (value.clamp(0.0, 1.0) * history as f64).round();
    if top >= history as f64 {
        history
    } else {
        // The value is clamped to 0..=1 and rounded, so it is a whole number of rows within the history.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a whole number in 0..history"
        )]
        let top = top as usize;
        top
    }
}

/// The knob's value and proportion for a view `offset` lines back from the live screen of a history of `history`
/// rows and a screen of `lines`.
fn knob(history: usize, offset: usize, lines: usize) -> (f64, f64) {
    let value = if history == 0 {
        1.0
    } else {
        history.saturating_sub(offset) as f64 / history as f64
    };
    let proportion = lines as f64 / (history + lines).max(1) as f64;
    (value, proportion)
}

/// The marks' places along `marks`' rows, one per kind where several would fall on the same of `steps` steps; in
/// drawing order, failed commands last so they stay on top.
fn places(marks: &ScrollMarks, steps: usize) -> Vec<(f64, ScrollMark)> {
    let steps = steps.max(1) as f64;
    let mut found: Vec<(ScrollMark, u64, f64)> = marks
        .marks
        .iter()
        .filter_map(|&(row, mark)| {
            let fraction = marks.fraction(row)?;
            // Steps are a few hundred at most, so the product fits a u64 exactly.
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "0..steps")]
            let step = (fraction * steps).floor() as u64;
            Some((mark, step, fraction))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let rows = marks.view.rows as f64;
    let last = (rows - 1.0).max(1.0) / rows.max(1.0);
    found
        .into_iter()
        .map(|(mark, _, fraction)| ((fraction / last).min(1.0), mark))
        .collect()
}

impl TerminalView {
    fn scrollbar(&self) -> Option<Retained<ScrollBar>> {
        self.subviews()
            .iter()
            .find_map(|view: Retained<NSView>| view.downcast::<ScrollBar>().ok())
    }

    fn make_scrollbar(&self) -> Retained<ScrollBar> {
        if let Some(bar) = self.scrollbar() {
            return bar;
        }
        let mtm = self.mtm();
        let width = NSScroller::scrollerWidthForControlSize_scrollerStyle(
            NSControlSize::Regular,
            NSScrollerStyle::Overlay,
            mtm,
        );
        let bounds = self.bounds();
        let frame = NSRect::new(
            NSPoint::new(bounds.size.width - width, 0.0),
            NSSize::new(width, bounds.size.height),
        );
        let bar = ScrollBar::new(mtm, self.id(), frame);
        self.addSubview(&bar);
        bar
    }

    /// The marks changed, such as for a new prompt or a finished command: the scrollbar reads them again.
    pub fn scroll_marks_changed(&self) {
        if let Some(bar) = self.scrollbar() {
            bar.ivars().stale.set(true);
        }
        self.update_scrollbar();
    }

    /// Follows the frame the view keeps: shows or hides the scrollbar, moves its knob and reads the marks again when
    /// they may have changed.
    pub(super) fn update_scrollbar(&self) {
        let Some((history, offset, lines)) = self
            .ivars()
            .screen
            .borrow()
            .as_ref()
            .map(|screen| (screen.history, screen.display_offset, screen.lines))
        else {
            return;
        };
        let bar = self.make_scrollbar();
        let ivars = bar.ivars();
        let now = Instant::now();
        if ivars.offset.replace(Some(offset)) != Some(offset) {
            ivars.linger_until.set(Some(now + LINGER));
        }
        let lingering = ivars.linger_until.get().is_some_and(|until| until > now)
            || NSScroller::preferredScrollerStyle(self.mtm()) == NSScrollerStyle::Legacy;
        if history == 0 || offset == 0 && !lingering {
            if !bar.isHidden() {
                bar.setHidden(true);
            }
            return;
        }
        if offset == 0 && ivars.linger_until.get().is_some_and(|until| until > now) && !ivars.pending.replace(true) {
            let id = self.id();
            let wait = ivars.linger_until.get().map_or(LINGER, |until| until - now);
            main_after(wait, move || {
                if let Some(view) = crate::find(id) {
                    if let Some(bar) = view.scrollbar() {
                        bar.ivars().pending.set(false);
                    }
                    view.update_scrollbar();
                }
            });
        }
        if bar.isHidden() {
            ivars.stale.set(true);
            bar.setHidden(false);
        }
        let background = self.ivars().theme.borrow().palette.background;
        let dark = u32::from(background.r) * 3 + u32::from(background.g) * 6 + u32::from(background.b) < 1280;
        let style = if dark {
            NSScrollerKnobStyle::Light
        } else {
            NSScrollerKnobStyle::Dark
        };
        if bar.knobStyle() != style {
            bar.setKnobStyle(style);
        }
        let (value, proportion) = knob(history, offset, lines);
        if (bar.doubleValue() - value).abs() > f64::EPSILON || (bar.knobProportion() - proportion).abs() > f64::EPSILON
        {
            bar.setDoubleValue(value);
            bar.setKnobProportion(proportion);
        }
        if ivars.stale.get() || ivars.read.get().is_none_or(|read| now - read >= MARKS_EVERY) {
            ivars.stale.set(false);
            ivars.read.set(Some(now));
            let Some(marks) = self.with_session(term_session::Session::scroll_marks) else {
                return;
            };
            let steps = (bar.bounds().size.height / MARK_HEIGHT).max(1.0);
            // A view is at most a few thousand points high.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a positive height"
            )]
            let steps = steps as usize;
            let placed = places(&marks, steps);
            if *ivars.marks.borrow() != placed {
                *ivars.marks.borrow_mut() = placed;
                NSView::setNeedsDisplay(&bar, true);
            }
        }
    }
}

#[cfg(feature = "selftest")]
impl TerminalView {
    /// The scrollbar as the self-test prints it: whether it shows, the knob, and how many marks of each kind.
    pub fn scrollbar_state(&self) -> String {
        let Some(bar) = self.scrollbar() else {
            return "none".into();
        };
        let marks = bar.ivars().marks.borrow();
        let count = |kind| marks.iter().filter(|(_, mark)| *mark == kind).count();
        format!(
            "{} value {:.3} knob {:.3}, marks: {} prompts, {} hits, {} failed",
            if bar.isHidden() { "hidden" } else { "shown" },
            bar.doubleValue(),
            bar.knobProportion(),
            count(ScrollMark::Prompt),
            count(ScrollMark::Hit),
            count(ScrollMark::Failed)
        )
    }
}

#[cfg(test)]
mod tests {
    use term_session::session::RowsView;

    use super::*;

    #[test]
    fn the_knob_follows_the_offset_and_shrinks_with_the_history() {
        assert_eq!(knob(0, 0, 24), (1.0, 1.0));
        assert_eq!(knob(76, 0, 24), (1.0, 0.24));
        assert_eq!(knob(76, 76, 24), (0.0, 0.24));
        assert_eq!(knob(100, 50, 100).0, 0.5);
    }

    #[test]
    fn a_knob_value_turns_back_into_the_row_at_the_top() {
        assert_eq!(top_for(0.0, 500), 0);
        assert_eq!(top_for(1.0, 500), 500);
        assert_eq!(top_for(0.5, 500), 250);
        assert_eq!(top_for(7.0, 500), 500);
        assert_eq!(top_for(-1.0, 500), 0);
    }

    #[test]
    fn marks_on_the_same_step_are_drawn_once_and_failures_last() {
        let marks = ScrollMarks {
            view: RowsView {
                first: 1000,
                rows: 101,
                top: 0,
                lines: 10,
                columns: 80,
            },
            marks: vec![
                (1100, ScrollMark::Failed),
                (1000, ScrollMark::Prompt),
                (1001, ScrollMark::Prompt),
                (1100, ScrollMark::Prompt),
                (999, ScrollMark::Hit),
                (1050, ScrollMark::Hit),
            ],
        };
        let placed = places(&marks, 10);
        assert_eq!(
            placed,
            [
                (0.0, ScrollMark::Prompt),
                (1.0, ScrollMark::Prompt),
                (0.5, ScrollMark::Hit),
                (1.0, ScrollMark::Failed)
            ]
        );
    }
}
