//! What the agent tab asks of the views around it: whether its window is in front, the home folder as `~`,
//! announcements for VoiceOver, and a watch that tells when a view comes into view and when it stayed out of it.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSAccessibilityPostNotificationWithUserInfo, NSApplication, NSView};
use objc2_foundation::{NSDictionary, NSNumber, NSObjectProtocol};
use term_ui::kit::{Observation, observe_window_visibility};

use super::constant;
use super::util::{guarded, ns};
pub use term_ui::kit::window_in_front as in_front;

/// `NSAccessibilityPriorityHigh`, which interrupts what VoiceOver is reading.
const HIGH_PRIORITY: i64 = 90;

/// `path` with the home folder written as `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let home = term_base::home();
    match path.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Has VoiceOver read `text` out, interrupting what it reads.
pub fn announce(mtm: MainThreadMarker, text: &str) {
    let app = NSApplication::sharedApplication(mtm);
    let message = ns(text);
    let priority = NSNumber::new_i64(HIGH_PRIORITY);
    let info = NSDictionary::<objc2_app_kit::NSAccessibilityNotificationUserInfoKey, AnyObject>::from_retained_objects(
        &[
            constant!(objc2_app_kit::NSAccessibilityAnnouncementKey),
            constant!(objc2_app_kit::NSAccessibilityPriorityKey),
        ],
        &[message.into(), priority.into()],
    );
    let notification = constant!(objc2_app_kit::NSAccessibilityAnnouncementRequestedNotification);
    // SAFETY: The application is an accessibility element, and the dictionary holds the announcement as a string and
    // its priority as a number, as the notification expects.
    unsafe { NSAccessibilityPostNotificationWithUserInfo(&app, notification, Some(&info)) };
}

fn out_of_view(view: &NSView) -> bool {
    view.isHiddenOrHasHiddenAncestor() || !view.window().is_some_and(|window| in_front(&window))
}

struct Watch {
    came: Box<dyn Fn()>,
    /// What runs once the view stayed out of view for the time given, in seconds.
    away: Option<(Box<dyn Fn()>, f64)>,
    visible: Cell<Option<bool>>,
    observations: RefCell<Vec<Observation>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `InViewWatch` does not implement Drop; objc2 drops its ivars
    // when the view is deallocated.
    /// An empty view that tells when it, or the view it sits in, comes into view and when it stayed out of it.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentInViewWatch"]
    #[ivars = Watch]
    struct InViewWatch;

    unsafe impl NSObjectProtocol for InViewWatch {}

    impl InViewWatch {
        #[unsafe(method(viewDidUnhide))]
        fn did_unhide(&self) {
            guarded((), || self.check_soon());
        }

        #[unsafe(method(viewDidHide))]
        fn did_hide(&self) {
            guarded((), || self.check_soon());
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            guarded((), || self.watch_window());
        }

        #[unsafe(method(termAgentCheckVisibility))]
        fn check_visibility(&self) {
            guarded((), || {
                let visible = !out_of_view(self);
                if self.ivars().visible.replace(Some(visible)) == Some(visible) {
                    return;
                }
                if visible {
                    self.cancel(sel!(termAgentStayedAway));
                    (self.ivars().came)();
                } else {
                    self.went();
                }
            });
        }

        #[unsafe(method(termAgentStayedAway))]
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
    fn watch_window(&self) {
        let mut observations = self.ivars().observations.borrow_mut();
        observations.clear();
        if let Some(window) = self.window() {
            let watch = Weak::from(self);
            *observations = observe_window_visibility(&window, move || {
                if let Some(watch) = watch.load() {
                    watch.check_soon();
                }
            });
        }
        self.check_soon();
    }

    /// AppKit may still be switching the selected window while posting the notification.
    fn check_soon(&self) {
        self.cancel(sel!(termAgentCheckVisibility));
        self.schedule(sel!(termAgentCheckVisibility), 0.0);
    }

    fn went(&self) {
        let Some((_, after)) = &self.ivars().away else { return };
        self.schedule(sel!(termAgentStayedAway), *after);
    }

    fn schedule(&self, selector: objc2::runtime::Sel, after: f64) {
        // SAFETY: The selector takes no argument and returns nothing; the run loop keeps the watch until it fires.
        let _: () =
            unsafe { msg_send![self, performSelector: selector, withObject: None::<&AnyObject>, afterDelay: after] };
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
    let mtm = MainThreadMarker::from(view);
    let watch = InViewWatch::alloc(mtm).set_ivars(Watch {
        came: Box::new(came),
        away: away.map(|(after, away)| (away, after.as_secs_f64())),
        visible: Cell::new(None),
        observations: RefCell::new(Vec::new()),
    });
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let watch: Retained<InViewWatch> = unsafe { msg_send![super(watch), init] };
    view.addSubview(&watch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_in_the_home_folder_start_with_a_tilde() {
        let home = term_base::home();
        assert_eq!(display_path(&home.join("Repos/x")), "~/Repos/x");
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(std::path::Path::new("/opt/x")), "/opt/x");
    }
}
