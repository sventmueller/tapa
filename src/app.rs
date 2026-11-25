//! The app's delegate: the first window at launch, a window when the Dock icon is clicked without one, what other
//! apps hand over (F216), asking before quitting (F213), the About panel, the Help menu and the menu actions.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSMenuItem, NSMenuItemValidation,
};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSURL};
use term_base::guarded;
use term_config::Action;

use crate::feedback::Page;
use crate::state::try_app;
use crate::window::Window;

/// What the delegate keeps.
pub(crate) struct Ivars {
    /// Whether another app handed something over at launch, which then opens instead of the first window.
    opened_at_launch: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AppDelegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAppDelegate"]
    #[ivars = Ivars]
    pub(crate) struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            guarded((), || {
                crate::window::spoken_tab::install();
                let restored = crate::restore::reopen();
                if !restored && !self.ivars().opened_at_launch.get() {
                    Window::open(None, None);
                }
                NSApplication::sharedApplication(self.mtm()).activate();
                crate::editor_tab::offer_recovered();
                crate::dictation::launched();
                #[cfg(feature = "selftest")]
                crate::selftest::start();
            });
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            guarded((), crate::window::app_activated);
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            guarded(NSApplicationTerminateReply::TerminateNow, || crate::quit::should_terminate(self.mtm()))
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last_window(&self, _sender: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _sender: &NSApplication, visible: bool) -> bool {
            guarded(true, || {
                let none = try_app().is_none_or(|app| {
                    app.windows
                        .borrow()
                        .iter()
                        .all(|window| window.is_quick() || window.is_asleep())
                });
                if !visible && none {
                    Window::open(None, None);
                }
                true
            })
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
            guarded((), || {
                self.ivars().opened_at_launch.set(true);
                for url in urls.iter() {
                    let Some(text) = url.absoluteString() else { continue };
                    if let Some(request) = crate::open::request(&text.to_string()) {
                        crate::open::carry_out(request);
                    }
                }
            });
        }
    }

    unsafe impl NSMenuItemValidation for AppDelegate {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(true, || crate::menu::validate(item))
        }
    }

    impl AppDelegate {
        #[unsafe(method(termAbout:))]
        fn about(&self, _sender: Option<&AnyObject>) {
            guarded((), || crate::about::show(self.mtm()));
        }

        #[unsafe(method(termReportProblem:))]
        fn report_problem(&self, _sender: Option<&AnyObject>) {
            guarded((), || crate::feedback::open(Page::Problem));
        }

        #[unsafe(method(termSuggestFeature:))]
        fn suggest_feature(&self, _sender: Option<&AnyObject>) {
            guarded((), || crate::feedback::open(Page::Idea));
        }

        #[unsafe(method(termOpenRepository:))]
        fn open_repository(&self, _sender: Option<&AnyObject>) {
            guarded((), || crate::feedback::open(Page::Repository));
        }

        #[unsafe(method(termAction:))]
        fn action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = sender
                    .and_then(|sender| sender.downcast_ref::<NSMenuItem>())
                    .map_or(-1, |item| item.tag());
                if let Some(action) = usize::try_from(tag).ok().and_then(|tag| Action::ALL.get(tag)) {
                    crate::menu::perform(*action);
                }
            });
        }
    }
);

impl AppDelegate {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<AppDelegate> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            opened_at_launch: Cell::new(false),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}
