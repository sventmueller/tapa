//! How a terminal view brings its screen up: a snapshot per frame of output, drawn with Core Graphics or on the GPU,
//! the blinking cursor, and the word to VoiceOver that the text changed.

use std::ptr::NonNull;
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAccessibilityValueChangedNotification, NSGraphicsContext, NSWindowOcclusionState};
use objc2_core_graphics::CGContext;
use objc2_foundation::{NSDate, NSObject, NSObjectProtocol, NSRect, NSRunLoop, NSTimer};
use objc2_quartz_core::CADisplayLink;
use splat_i18n::t;
use term_session::{Damage, Session, Snapshot};

use super::view::TerminalView;
use crate::kit::guarded;

/// How long after a redraw event the session may send the next: a frame at 120 Hz.
const REARM_AFTER: std::time::Duration = std::time::Duration::from_millis(8);
use crate::model::{self, Frame};
use term_render::constant;
use term_render::palette::Theme;
use term_render::render::{self, Overlay};

/// How long the cursor shows and hides while it blinks, in seconds.
const BLINK: f64 = 0.53;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `FrameTarget` does not implement Drop.
    /// The target of a terminal view's display link, which finds the view by its id so the link does not retain it.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "IDETerminalFrameTarget"]
    #[ivars = u64]
    struct FrameTarget;

    unsafe impl NSObjectProtocol for FrameTarget {}

    impl FrameTarget {
        #[unsafe(method(ideTerminalFrame:))]
        fn frame(&self, _link: &CADisplayLink) {
            guarded((), || {
                if let Some(view) = super::find(*self.ivars()) {
                    view.display_frame();
                }
            });
        }
    }
);

impl FrameTarget {
    fn new(mtm: MainThreadMarker, id: u64) -> Retained<FrameTarget> {
        let this = Self::alloc(mtm).set_ivars(id);
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

impl TerminalView {
    /// Output arrived; a new snapshot follows at once or with the display's next frame, unless the view is out of
    /// sight.
    pub(super) fn redraw_requested(&self) {
        if self.has_renderer() {
            return self.frame_published();
        }
        self.raw_suggest();
        if self.ivars().metal.supported {
            self.ivars().state.stale.set(true);
            return;
        }
        let state = &self.ivars().state;
        let hidden = self.isHiddenOrHasHiddenAncestor() || !self.on_display();
        match state.frame(Instant::now(), hidden) {
            Frame::Stale | Frame::Pending => {}
            Frame::Now => self.refresh(state.stale.get()),
            Frame::After(_) => self.run_frames(),
        }
    }

    /// Whether some part of the window shows on screen; the display link does not fire while none does.
    fn on_display(&self) -> bool {
        self.window()
            .is_some_and(|window| window.occlusionState().contains(NSWindowOcclusionState::Visible))
    }

    fn run_frames(&self) {
        let mut link = self.ivars().frames.borrow_mut();
        let link = link.get_or_insert_with(|| {
            let target = FrameTarget::new(self.mtm(), self.id());
            // SAFETY: `ideTerminalFrame:` takes the link as its one argument; the target holds only the view's id, and
            // the view invalidates the link when it goes.
            let link = unsafe { self.displayLinkWithTarget_selector(&target, sel!(ideTerminalFrame:)) };
            let modes = constant!(objc2_foundation::NSRunLoopCommonModes);
            // SAFETY: The main run loop in its common modes, so output still shows while the user drags a size.
            unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), modes) };
            link
        });
        if link.isPaused() {
            link.setPaused(false);
        }
    }

    pub(super) fn pause_frames(&self) {
        if let Some(link) = self.ivars().frames.borrow().as_ref() {
            link.setPaused(true);
        }
    }

    /// A frame of the display: takes the snapshot that waits, or pauses the link once none does.
    fn display_frame(&self) {
        let state = &self.ivars().state;
        if !state.frame_pending.replace(false) {
            return self.pause_frames();
        }
        if self.isHiddenOrHasHiddenAncestor() {
            state.stale.set(true);
            return self.pause_frames();
        }
        self.refresh(state.stale.get());
    }

    /// Takes the snapshot that output out of sight left waiting, before the view draws a picture of itself.
    pub(super) fn catch_up(&self) {
        let state = &self.ivars().state;
        if self.ivars().metal.supported && !self.has_renderer() {
            *self.ivars().screen.borrow_mut() = self.with_session(Session::snapshot).map(std::sync::Arc::new);
            return;
        }
        if state.stale.get() || state.frame_pending.get() {
            self.refresh(true);
        }
    }

    /// Takes a new snapshot and draws it, after the app scrolled the session.
    pub fn refresh_now(&self) {
        self.refresh(true);
    }

    /// Publishes mouse-driven viewport changes without building a snapshot in the event handler.
    pub(crate) fn refresh_interactive(&self) {
        if !self.with_session(Session::request_frame).unwrap_or(false) {
            self.refresh(true);
        }
    }

    /// The reader thread published a frame, which the render thread draws on its own (E5): the view keeps it for
    /// what it reads of the screen, such as links and VoiceOver, and follows the cursor's blinking.
    fn frame_published(&self) {
        if self.take_latest_frame() && self.ivars().state.exited.get() {
            self.repaint();
        }
        self.field_follow_screen();
        self.raw_suggest();
        // Re-armed a display frame later rather than at once, so the reader thread wakes the main thread at most that
        // often while output streams; the frame is taken again then, so the last one is never missed.
        if !self.ivars().rearm_pending.replace(true) {
            let id = self.id();
            term_base::main_after(REARM_AFTER, move || {
                if let Some(view) = crate::host::find(id) {
                    view.ivars().rearm_pending.set(false);
                    view.with_session(Session::rearm);
                    if view.take_latest_frame() {
                        view.raw_suggest();
                        if view.ivars().state.exited.get() {
                            view.repaint();
                        }
                        view.update_blink();
                        view.announce();
                        view.update_scrollbar();
                        view.copy_output();
                        view.blocks_follow();
                    }
                }
            });
        }
        if let Some(typed) = self.ivars().keys.typed.borrow_mut().take() {
            drop(typed);
        }
        self.update_blink();
        self.announce();
        self.update_scrollbar();
        self.copy_output();
        self.blocks_follow();
    }

    /// Keeps the session's latest frame for what the view reads of the screen; whether it was new.
    fn take_latest_frame(&self) -> bool {
        let Some(Some(frame)) = self.with_session(Session::latest) else {
            return false;
        };
        let mut screen = self.ivars().screen.borrow_mut();
        let new = screen.as_ref().is_none_or(|old| !std::sync::Arc::ptr_eq(old, &frame));
        *screen = Some(frame);
        self.ivars().state.stale.set(false);
        new
    }

    /// Draws again what lies over the frame, after the focus, the cursor's blinking, composed text, the bell or the
    /// program's end changed it.
    pub(super) fn repaint(&self) {
        if self.has_renderer() {
            let overlay = self.overlay_state();
            self.with_renderer(|renderer| renderer.set_overlay(overlay));
        } else if !self.ivars().metal.supported {
            self.setNeedsDisplay(true);
        }
    }

    /// What lies over the frame now, for the render thread.
    fn overlay_state(&self) -> term_render::thread::OverlayState {
        let mut state = term_render::thread::OverlayState::default();
        self.with_frame(|_, _, overlay| {
            state = term_render::thread::OverlayState {
                focused: overlay.focused,
                cursor_on: overlay.cursor_on,
                hide_cursor: overlay.hide_cursor,
                marked: overlay.marked.to_string(),
                ghost: overlay.ghost.map(|(point, text)| (point, text.to_string())),
                final_line: overlay.final_line.map(|(line, text)| (line, text.to_string())),
                flash: overlay.flash,
            };
        });
        state
    }

    /// Takes a snapshot of the screen and marks the lines that changed for drawing; `all` marks every line. With the
    /// render thread, the reader thread's frame is built anew instead, as the owner scrolled, selected or resized.
    pub(super) fn refresh(&self, all: bool) {
        if self.has_renderer() {
            self.with_session(Session::republish);
            self.frame_published();
            self.repaint();
            return;
        }
        if self.ivars().metal.supported {
            self.ivars().state.stale.set(true);
            return;
        }
        self.field_follow_screen();
        self.raw_suggest();
        let Some(screen) = self.with_session(Session::snapshot) else {
            self.setNeedsDisplay(true);
            return;
        };
        self.ivars().state.framed(Instant::now());
        let keys = &self.ivars().keys;
        if let Some(typed) = keys.typed.borrow_mut().take() {
            *keys.typed_shown.borrow_mut() = Some(typed);
        }
        let m = self.metrics();
        let width = self.bounds().size.width;
        let previous = self.ivars().screen.borrow_mut().replace(std::sync::Arc::new(screen));
        {
            let screen = self.ivars().screen.borrow();
            let Some(now) = screen.as_ref() else { return };
            let same_shape = previous
                .as_ref()
                .is_some_and(|before| before.lines == now.lines && before.columns == now.columns);
            let previous = previous.as_deref();
            match &now.damage {
                Damage::Lines(lines) if same_shape && !all => {
                    let before = previous.map_or(now.cursor.point.line, |b| b.cursor.point.line);
                    let changed = lines.iter().map(|line| line.line);
                    let selected = model::selection_lines(previous.and_then(|b| b.selection), now.selection);
                    for line in changed.chain([before, now.cursor.point.line]).chain(selected) {
                        self.setNeedsDisplayInRect(m.line_rect(line, width));
                    }
                }
                _ => self.setNeedsDisplay(true),
            }
        }
        self.update_blink();
        self.announce();
        self.update_scrollbar();
        self.blocks_follow();
    }

    /// Draws with Core Graphics, where there is no GPU and for pictures of the view.
    pub(super) fn draw(&self, dirty: NSRect) {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("terminal frame"));
        let Some(context) = NSGraphicsContext::currentContext() else {
            return;
        };
        let ctx = context.CGContext();
        // Drawing the window into a bitmap passes a rectangle beyond the view and does not clip to it.
        let bounds = self.bounds();
        CGContext::clip_to_rect(Some(&ctx), bounds);
        let dirty = render::intersect(dirty, bounds);
        let fonts = self.fonts();
        self.with_frame(|theme, screen, overlay| match screen {
            Some(screen) => render::draw(&ctx, &fonts, theme, screen, overlay, dirty),
            None => render::fill(&ctx, theme.palette.background, dirty),
        });
        drop(self.ivars().keys.typed_shown.borrow_mut().take());
    }

    /// Runs `f` with what a frame shows: the colours, the screen once the session has started, and what lies over it.
    fn with_frame(&self, f: impl FnOnce(&Theme, Option<&Snapshot>, &Overlay)) {
        let theme = self.ivars().theme.borrow();
        let screen = self.ivars().screen.borrow();
        let screen = screen.as_deref();
        let state = &self.ivars().state;
        let marked = self.ivars().keys.marked.borrow();
        let ghost = self.ivars().ghost.borrow();
        let final_line = screen.and_then(|screen| {
            (state.exited.get() && screen.display_offset == 0)
                .then(|| (model::final_line(screen), t("[Prozess beendet]")))
        });
        let overlay = Overlay {
            focused: state.focused.get(),
            cursor_on: !state.cursor_off.get(),
            hide_cursor: self.in_copy_mode(),
            marked: &marked,
            ghost: ghost.as_ref().map(|ghost| (ghost.point, ghost.shown.as_str())),
            final_line,
            flash: state.flashing(Instant::now()),
        };
        f(&theme, screen, &overlay);
    }

    /// The render thread's layer follows a change of size or scale, which Core Graphics drawing gets from AppKit.
    pub(super) fn redraw_layer(&self) {
        if self.has_renderer() {
            let geometry = term_render::thread::Geometry {
                frame: self.bounds(),
                scale: self.window().map_or(2.0, |window| window.backingScaleFactor()),
            };
            self.with_renderer(|renderer| renderer.set_geometry(geometry));
            let _ = self.fonts();
        }
    }

    /// Draws what arrived while the window was hidden behind others, once some of it shows again.
    pub(super) fn occlusion_changed(&self) {
        self.update_surface_visibility();
        let state = &self.ivars().state;
        if !self.on_display() {
            return;
        }
        if state.size_waits.get() {
            self.layout_grid();
        }
        if self.ivars().metal.supported {
            self.repaint();
        } else if state.stale.get() && !self.isHiddenOrHasHiddenAncestor() {
            self.refresh(true);
        } else if state.unpresented.get() {
            self.setNeedsDisplay(true);
        }
    }

    /// Starts or stops the cursor's blinking as the program asks while the view has the focus.
    pub(super) fn update_blink(&self) {
        let state = &self.ivars().state;
        let cursor_blinks = self
            .ivars()
            .screen
            .borrow()
            .as_ref()
            .is_some_and(|screen| screen.cursor.blinking && screen.cursor.visible);
        let blinking = state.blinking(cursor_blinks);
        let mut timer = self.ivars().blink.borrow_mut();
        match (blinking, timer.is_some()) {
            (true, false) => {
                let view = Weak::from(self);
                let block = RcBlock::new(move |_: NonNull<NSTimer>| {
                    guarded((), || {
                        if let Some(view) = view.load() {
                            let off = &view.ivars().state.cursor_off;
                            off.set(!off.get());
                            view.invalidate_cursor();
                        }
                    });
                });
                // SAFETY: The block holds the view weakly; the main run loop fires the timer on the main thread.
                *timer = Some(unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(BLINK, true, &block) });
            }
            (false, true) => {
                if let Some(timer) = timer.take() {
                    timer.invalidate();
                }
                state.cursor_off.set(false);
            }
            _ => {}
        }
    }

    /// Shows the cursor at once after a key, and keeps it shown for a full blink.
    pub(super) fn wake_cursor(&self) {
        if self.ivars().state.cursor_off.replace(false) {
            self.invalidate_cursor();
        }
        if let Some(timer) = self.ivars().blink.borrow().as_ref() {
            timer.setFireDate(&NSDate::dateWithTimeIntervalSinceNow(BLINK));
        }
    }

    pub(super) fn invalidate_cursor(&self) {
        if self.ivars().metal.supported {
            return self.repaint();
        }
        let line = self
            .ivars()
            .screen
            .borrow()
            .as_ref()
            .map(|screen| screen.cursor.point.line);
        if let Some(line) = line {
            let width = self.bounds().size.width;
            self.setNeedsDisplayInRect(self.metrics().line_rect(line, width));
        }
    }

    /// Lets VoiceOver know the text changed, at most every half second.
    fn announce(&self) {
        if !self.ivars().state.announce(Instant::now()) {
            return;
        }
        let element: &AnyObject = self;
        let changed = constant!(NSAccessibilityValueChangedNotification);
        // SAFETY: The element is this view and the name AppKit's value-changed notification.
        unsafe { objc2_app_kit::NSAccessibilityPostNotification(element, changed) };
    }
}
