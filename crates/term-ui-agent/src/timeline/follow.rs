//! Following the end of the timeline: the user's scrolling switches it off and on, and new content glides into view
//! frame by frame instead of jumping there, within `GLIDE` and only until the user scrolls or types.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::sel;
use objc2_app_kit::{NSApplication, NSEventType, NSScrollView, NSScrollerStyle, NSWindowOcclusionState, NSWorkspace};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRunLoop};
use objc2_quartz_core::CADisplayLink;

use super::Timeline;
use crate::common::constant;
use crate::common::util::{FlagGuard, guarded};

/// How quickly a glide closes its distance to the end: the time for it to shrink to about a third.
const TAU: f64 = 0.06;
/// How close to the end the user has to scroll for the timeline to follow it again.
const REATTACH: f64 = 16.0;
/// The longest a glide takes; it lands on the end when the time is up (SN-W3).
const GLIDE: f64 = 0.15;

/// Whether the timeline follows its end, as the user's scrolling decided.
pub(super) struct Follow {
    on: Cell<bool>,
    /// The scroll position the bounds observer saw last.
    last: Cell<f64>,
    /// Set while the timeline scrolls itself, which must not count as the user scrolling away.
    ours: Cell<bool>,
    /// Set when the user's input moved the view during a glide, which ends it.
    interrupted: Cell<bool>,
}

impl Follow {
    pub(super) fn new() -> Rc<Follow> {
        Rc::new(Follow {
            on: Cell::new(true),
            last: Cell::new(0.0),
            ours: Cell::new(false),
            interrupted: Cell::new(false),
        })
    }

    pub(super) fn on(&self) -> bool {
        self.on.get()
    }
}

/// The glide's display link and the times of its first and last frame.
#[derive(Default)]
pub(super) struct Glide {
    link: RefCell<Option<Retained<CADisplayLink>>>,
    started: Cell<f64>,
    last: Cell<f64>,
}

impl Drop for Glide {
    fn drop(&mut self) {
        // The link retains its target, the tab's `AgentTarget`, until it is invalidated.
        if let Some(link) = self.link.get_mut().take() {
            link.invalidate();
        }
    }
}

/// Whether the timeline follows its end after the user moved it from `last` to `y`, with its end at `end`.
fn decide(on: bool, last: f64, y: f64, end: f64) -> bool {
    // A shrinking table pulls the view up only as far as its end, so moving up and away is the user.
    if y < last - 0.5 && y < end - 1.0 {
        return false;
    }
    if y >= last - 0.5 && y >= end - REATTACH {
        return true;
    }
    on
}

/// Where a glide from `y` to `end` stands `dt` seconds later, with `left` seconds of it to go.
fn step(y: f64, end: f64, dt: f64, left: f64) -> f64 {
    if end <= y || dt >= left {
        return end;
    }
    let eased = (-dt.clamp(0.0, 0.1) / TAU).exp();
    // Moving at least the share of the time left keeps the glide within `GLIDE`.
    let next = end - (end - y) * eased.min(1.0 - dt / left);
    if end - next < 0.5 { end } else { next }
}

/// Whether the event AppKit handles now is the user's own input, which moves the view in place of a glide.
fn by_user(scroll: &NSScrollView) -> bool {
    let event = NSApplication::sharedApplication(MainThreadMarker::from(scroll)).currentEvent();
    event.is_some_and(|event| {
        matches!(
            event.r#type(),
            NSEventType::ScrollWheel
                | NSEventType::KeyDown
                | NSEventType::LeftMouseDown
                | NSEventType::LeftMouseDragged
        )
    })
}

fn end_of(scroll: &NSScrollView) -> f64 {
    let clip = scroll.contentView().bounds();
    let height = scroll.documentView().map_or(0.0, |table| table.frame().size.height);
    (height - clip.size.height).max(0.0)
}

/// Tracks the user's scrolling, and gives the timeline its scroller back once it no longer follows its end; following
/// hides an overlay scroller, which would otherwise show itself at each streamed line.
pub(super) fn observe(scroll: &NSScrollView, follow: &Rc<Follow>) -> Retained<ProtocolObject<dyn NSObjectProtocol>> {
    let clip = scroll.contentView();
    clip.setPostsBoundsChangedNotifications(true);
    let weak = objc2::rc::Weak::from(scroll);
    let follow = follow.clone();
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        guarded((), || {
            let Some(scroll) = weak.load() else { return };
            let y = scroll.contentView().bounds().origin.y;
            if !follow.ours.get() {
                follow
                    .on
                    .set(decide(follow.on.get(), follow.last.get(), y, end_of(&scroll)));
                if by_user(&scroll) {
                    follow.interrupted.set(true);
                }
            }
            follow.last.set(y);
            if !follow.on.get() && !scroll.hasVerticalScroller() {
                scroll.setHasVerticalScroller(true);
            }
        });
    });
    let name = constant!(objc2_app_kit::NSViewBoundsDidChangeNotification);
    // SAFETY: The block holds the scroll view weakly and does nothing once it is gone; the timeline removes the
    // observer when it goes.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            Some(&clip),
            None,
            &block,
        )
    }
}

impl Timeline {
    /// Whether the timeline follows its end.
    pub fn follows(&self) -> bool {
        self.follow.on()
    }

    /// Scrolls the view to `y` as the timeline's own move.
    pub(super) fn scroll_ours(&self, y: f64) {
        let clip = self.scroll.contentView();
        let _ours = FlagGuard::raise(&self.follow.ours);
        clip.scrollToPoint(NSPoint::new(0.0, y));
        self.scroll.reflectScrolledClipView(&clip);
    }

    /// Shifts the view by `by` points to keep the rows in view where they were.
    pub(super) fn shift_by(&self, by: f64) {
        let y = self.scroll.contentView().bounds().origin.y;
        self.scroll_ours((y + by).max(0.0));
    }

    /// Shows the end at once and follows it from now on.
    pub fn scroll_to_end(&self) {
        self.follow.on.set(true);
        self.stop_glide();
        if self.count() == 0 {
            return;
        }
        // After a reload or height change the table sizes itself only on its next layout pass, too late to find the end.
        self.table.tile();
        self.hide_overlay_scroller();
        self.scroll_ours(end_of(&self.scroll));
    }

    /// Brings new content at the end into view, gliding there when the timeline is on screen.
    pub(super) fn follow_end(&self) {
        if !self.follow.on() || self.count() == 0 {
            return;
        }
        if !self.is_visible()
            || !self.on_display()
            || NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
        {
            return self.scroll_to_end();
        }
        self.table.tile();
        self.hide_overlay_scroller();
        let clip = self.scroll.contentView().bounds();
        let distance = end_of(&self.scroll) - clip.origin.y;
        if distance < 0.5 {
            return;
        }
        // Gliding through more than a screen of new rows would only blur past them and leave the end behind.
        if distance > clip.size.height {
            return self.scroll_to_end();
        }
        let mut link = self.glide.link.borrow_mut();
        let link = link.get_or_insert_with(|| {
            // SAFETY: `ideAgentGlide:` takes the link as its one argument; the glide invalidates the link, which
            // retains the target, when the timeline goes.
            let link = unsafe {
                self.scroll
                    .displayLinkWithTarget_selector(&self.target, sel!(ideAgentGlide:))
            };
            let modes = constant!(objc2_foundation::NSRunLoopCommonModes);
            // SAFETY: The main run loop in its common modes, so the glide goes on while the user drags a scroller.
            unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), modes) };
            link
        });
        if link.isPaused() {
            self.follow.interrupted.set(false);
            link.setPaused(false);
        }
    }

    /// Moves a glide on by one frame of `link`.
    pub(crate) fn glide_frame(&self, link: &CADisplayLink) {
        if !self.follow.on() || !self.is_visible() || self.follow.interrupted.get() {
            return self.stop_glide();
        }
        let now = link.timestamp();
        let last = self.glide.last.replace(now);
        let dt = if last == 0.0 { link.duration() } else { now - last };
        if last == 0.0 {
            self.glide.started.set(now - dt);
        }
        let left = GLIDE - (last.max(self.glide.started.get()) - self.glide.started.get());
        let y = self.scroll.contentView().bounds().origin.y;
        let end = end_of(&self.scroll);
        let next = step(y, end, dt, left);
        self.scroll_ours(next);
        if next >= end {
            self.stop_glide();
        }
    }

    /// Whether some part of the window shows on screen; the display link does not fire while none does, as on a
    /// locked screen, and a glide would stop short of the end.
    fn on_display(&self) -> bool {
        self.scroll
            .window()
            .is_some_and(|window| window.occlusionState().contains(NSWindowOcclusionState::Visible))
    }

    fn stop_glide(&self) {
        if let Some(link) = self.glide.link.borrow().as_ref() {
            link.setPaused(true);
        }
        self.glide.last.set(0.0);
        self.glide.started.set(0.0);
    }

    fn hide_overlay_scroller(&self) {
        if self.scroll.scrollerStyle() == NSScrollerStyle::Overlay && self.scroll.hasVerticalScroller() {
            self.scroll.setHasVerticalScroller(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_up_away_from_the_end_stops_following() {
        assert!(!decide(true, 1000.0, 990.0, 1000.0));
        assert!(!decide(true, 500.0, 499.0, 1000.0));
    }

    #[test]
    fn a_shrinking_table_that_pulls_the_view_to_its_end_keeps_following() {
        assert!(decide(true, 1000.0, 800.0, 800.0));
        assert!(decide(true, 1000.0, 1100.0, 1000.0), "the rubber band past the end");
    }

    #[test]
    fn scrolling_back_down_to_the_end_follows_again() {
        assert!(!decide(false, 500.0, 900.0, 1000.0));
        assert!(decide(false, 900.0, 990.0, 1000.0));
        assert!(decide(false, 1000.0, 1000.0, 1000.0));
    }

    #[test]
    fn scrolling_down_while_following_keeps_following() {
        assert!(decide(true, 600.0, 700.0, 1000.0));
    }

    #[test]
    fn a_glide_closes_most_of_its_distance_within_a_few_frames_and_lands_on_the_end() {
        let frame = 1.0 / 60.0;
        let mut y = 0.0;
        for _ in 0..6 {
            y = step(y, 100.0, frame, 1.0);
        }
        assert!(y > 75.0 && y < 100.0, "{y}");
        for _ in 0..60 {
            y = step(y, 100.0, frame, 1.0);
        }
        assert_eq!(y, 100.0);
    }

    #[test]
    fn a_glide_over_a_screen_lands_on_the_end_within_its_time() {
        let frame = 1.0 / 60.0;
        let (mut y, mut spent) = (0.0, 0.0);
        while y < 1000.0 {
            y = step(y, 1000.0, frame, GLIDE - spent);
            spent += frame;
            assert!(spent <= GLIDE + frame, "{spent} s for {y}");
        }
        assert!(spent > GLIDE / 2.0, "it still glides: {spent} s");
    }

    #[test]
    fn a_glide_never_passes_the_end_and_survives_a_long_pause() {
        assert_eq!(step(120.0, 100.0, 0.016, 1.0), 100.0);
        let after_pause = step(0.0, 100.0, 5.0, 10.0);
        assert!(after_pause < 100.0 && after_pause > 80.0, "{after_pause}");
    }
}
