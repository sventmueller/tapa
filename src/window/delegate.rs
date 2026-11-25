//! The delegate of each of the app's `NSWindow`s: closing asks first while programs run (F213), a tab's window that
//! closes takes its tab with it, and the window that comes to the front tells its `Window`, whose tab in front has
//! then been seen.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSWindow, NSWindowDelegate};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol};
use term_base::guarded;

use crate::state::try_app;

/// Which window an `NSWindow` shows, and which of its tabs; none for the quick terminal's panel.
pub(crate) struct Ivars {
    pub(crate) window: u64,
    pub(crate) tab: Option<u64>,
    /// Set once the user agreed to close, so the second `windowShouldClose:` does not ask again.
    pub(crate) agreed: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermWindowDelegate"]
    #[ivars = Ivars]
    pub(crate) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            guarded(true, || self.may_close())
        }

        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            guarded((), || self.closing());
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _notification: &NSNotification) {
            guarded((), || self.became_key());
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn did_end_live_resize(&self, _notification: &NSNotification) {
            guarded((), || {
                if let (Some(window), Some(tab)) = (
                    try_app().and_then(|app| app.window(self.ivars().window)),
                    self.ivars().tab,
                ) {
                    window.sync_group(tab);
                }
            });
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _notification: &NSNotification) {
            guarded((), || {
                if let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) {
                    window.resigned_key();
                }
            });
        }
    }
);

impl Delegate {
    pub(crate) fn new(mtm: MainThreadMarker, window: u64, tab: Option<u64>) -> Retained<Delegate> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            window,
            tab,
            agreed: Cell::new(false),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }

    fn may_close(&self) -> bool {
        if self.ivars().agreed.get() {
            return true;
        }
        let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) else {
            return true;
        };
        let agreed = match self.ivars().tab {
            Some(tab) => window.may_close_tab(tab),
            None => window.may_close_window(),
        };
        self.ivars().agreed.set(agreed);
        agreed
    }

    fn closing(&self) {
        let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) else {
            return;
        };
        match self.ivars().tab {
            Some(tab) => window.tab_window_closed(tab),
            None => window.closed(),
        }
    }

    fn became_key(&self) {
        let Some(window) = try_app().and_then(|app| app.window(self.ivars().window)) else {
            return;
        };
        match self.ivars().tab {
            Some(tab) => window.tab_became_key(tab),
            None => window.front_seen(),
        }
    }
}
