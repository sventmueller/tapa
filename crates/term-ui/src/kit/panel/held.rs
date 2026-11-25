//! Views that know when they come into view and when they have stayed out of it: lists that load anew only while
//! they are in view, and views that let go of what they built while out of it.
//!
//! On macOS 27 every `reloadData` of an outline view leaves the disclosure button of each expandable row it shows
//! behind for good, so a list that reloads out of sight piles them up for nothing.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSView, NSWindowDidBecomeKeyNotification, NSWindowDidBecomeMainNotification,
    NSWindowDidChangeOcclusionStateNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};

use crate::kit::guarded;

/// Whether `view` is out of view: in no window, or hidden itself or by an ancestor.
pub fn out_of_view(view: &NSView) -> bool {
    view.window().is_none() || view.isHiddenOrHasHiddenAncestor()
}

struct Watch {
    came: Box<dyn Fn()>,
    /// What runs once the view stayed out of view for the time given, in seconds.
    away: Option<(Box<dyn Fn()>, f64)>,
    /// `came` runs once, when the view is in view in the window tab in front of its tab group, and the watch goes.
    once_in_front: bool,
    /// The notifications of its window a watch `once_in_front` follows, since a tab group selecting another window
    /// changes no view.
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        stop_observing(self.observers.get_mut());
    }
}

fn stop_observing(observers: &mut Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>) {
    let center = NSNotificationCenter::defaultCenter();
    for token in observers.drain(..) {
        // SAFETY: The token came from this center's block-based `addObserverForName`.
        unsafe { center.removeObserver(token.as_ref()) };
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `InViewWatch` does not implement Drop; its ivars do, and
    // objc2 drops them when the view is deallocated.
    /// An empty view that tells when it, or the view it sits in, comes into view and when it stayed out of it.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermInViewWatch"]
    #[ivars = Watch]
    struct InViewWatch;

    unsafe impl NSObjectProtocol for InViewWatch {}

    impl InViewWatch {
        #[unsafe(method(viewDidUnhide))]
        fn did_unhide(&self) {
            guarded((), || self.came());
        }

        #[unsafe(method(viewDidHide))]
        fn did_hide(&self) {
            guarded((), || self.went());
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            guarded((), || {
                if self.ivars().once_in_front {
                    self.follow_window();
                }
                if self.window().is_some() {
                    self.came();
                } else {
                    self.went();
                }
            });
        }

        #[unsafe(method(termCameIntoView))]
        fn came_into_view(&self) {
            guarded((), || {
                if out_of_view(self) {
                    return;
                }
                if !self.ivars().once_in_front {
                    return (self.ivars().came)();
                }
                if self.window().is_some_and(|window| super::in_front(&window)) {
                    stop_observing(&mut self.ivars().observers.borrow_mut());
                    self.removeFromSuperview();
                    (self.ivars().came)();
                }
            });
        }

        #[unsafe(method(termStayedAway))]
        fn stayed_away(&self) {
            guarded((), || {
                if let Some((away, _)) = &self.ivars().away
                    && out_of_view(self)
                {
                    away();
                }
            });
        }
    }
);

impl InViewWatch {
    /// Runs `came` once the change of the view hierarchy that brought the view into view is done.
    fn came(&self) {
        self.cancel(sel!(termStayedAway));
        self.cancel(sel!(termCameIntoView));
        // SAFETY: `termCameIntoView` takes no argument and returns nothing; the run loop keeps the watch until it fires.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(termCameIntoView), withObject: None::<&AnyObject>, afterDelay: 0.0]
        };
    }

    fn went(&self) {
        self.cancel(sel!(termCameIntoView));
        let Some((_, after)) = &self.ivars().away else { return };
        self.cancel(sel!(termStayedAway));
        // SAFETY: `termStayedAway` takes no argument and returns nothing; the run loop keeps the watch until it fires.
        let _: () = unsafe {
            msg_send![self, performSelector: sel!(termStayedAway), withObject: None::<&AnyObject>, afterDelay: *after]
        };
    }

    /// Looks again whether the view shows each time its window becomes key or main or comes on screen.
    fn follow_window(&self) {
        stop_observing(&mut self.ivars().observers.borrow_mut());
        let Some(window) = self.window() else { return };
        let center = NSNotificationCenter::defaultCenter();
        for name in [
            term_render::constant!(NSWindowDidBecomeKeyNotification),
            term_render::constant!(NSWindowDidBecomeMainNotification),
            term_render::constant!(NSWindowDidChangeOcclusionStateNotification),
        ] {
            let watch = Weak::from(self);
            let block = RcBlock::new(move |_: NonNull<NSNotification>| {
                guarded((), || {
                    if let Some(watch) = watch.load() {
                        watch.came();
                    }
                });
            });
            // SAFETY: The block holds the watch weakly and runs on the main thread, where windows post these.
            let token =
                unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), Some(&window), None, &block) };
            self.ivars().observers.borrow_mut().push(token);
        }
    }

    fn cancel(&self, selector: objc2::runtime::Sel) {
        // SAFETY: NSObject's class method, with the target, selector and argument the watch scheduled.
        let _: () = unsafe {
            msg_send![NSObject::class(), cancelPreviousPerformRequestsWithTarget: self, selector: selector, object: None::<&AnyObject>]
        };
    }
}

/// Puts a watch into `view` that runs `came` whenever `view` or a view around it comes into view, and `away` once it
/// has stayed out of view for `after`.
pub fn watch_view(view: &NSView, came: impl Fn() + 'static, away: Option<(Duration, Box<dyn Fn()>)>) {
    add_watch(
        view,
        Watch {
            came: Box::new(came),
            away: away.map(|(after, away)| (away, after.as_secs_f64())),
            once_in_front: false,
            observers: RefCell::new(Vec::new()),
        },
    );
}

/// Runs `shown` once, the first time `view` is in view in the window tab in front of its tab group, as work a tab
/// restored behind others leaves until the user looks at it.
pub fn when_first_shown(view: &NSView, shown: impl FnOnce() + 'static) {
    let shown = Cell::new(Some(shown));
    add_watch(
        view,
        Watch {
            came: Box::new(move || {
                if let Some(shown) = shown.take() {
                    shown();
                }
            }),
            away: None,
            once_in_front: true,
            observers: RefCell::new(Vec::new()),
        },
    );
}

fn add_watch(view: &NSView, watch: Watch) {
    let mtm = MainThreadMarker::from(view);
    let watch = InViewWatch::alloc(mtm).set_ivars(watch);
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let watch: Retained<InViewWatch> = unsafe { msg_send![super(watch), init] };
    view.addSubview(&watch);
}

/// Whether a list missed a reload while it was out of view.
#[derive(Default)]
pub struct HeldReload {
    stale: Cell<bool>,
}

impl HeldReload {
    /// Runs `came` each time `page` or a view around it comes into view; `came` should catch up with
    /// [`HeldReload::catch_up`].
    pub fn watch(page: &NSView, came: impl Fn() + 'static) {
        watch_view(page, came, None);
    }

    /// Whether a reload of the list in `view` waits, because `view` is out of view; it then runs on coming back.
    pub fn holds(&self, view: &NSView) -> bool {
        let away = out_of_view(view);
        self.stale.set(away);
        away
    }

    /// Whether the list in `view` missed a reload and is in view again, so it should reload now.
    pub fn catch_up(&self, view: &NSView) -> bool {
        self.stale.get() && !out_of_view(view)
    }
}
