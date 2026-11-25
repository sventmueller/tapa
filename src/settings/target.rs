//! The object AppKit talks to in the settings window: the controls' actions, the toolbar's items and the window's
//! closing. Each call hands over to the open window's state.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSControl, NSToolbar, NSToolbarDelegate, NSToolbarItem, NSWindowDelegate};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSString};
use term_base::guarded;

use super::{Pane, with_open};

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Target` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSettingsTarget"]
    pub(crate) struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSWindowDelegate for Target {
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            guarded((), super::closed);
        }
    }

    unsafe impl NSToolbarDelegate for Target {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, identifier: &NSString, _flag: bool) -> Option<Retained<NSToolbarItem>> {
            guarded(None, || {
                let pane = Pane::ALL.iter().find(|pane| pane.id() == identifier.to_string())?;
                Some(super::toolbar_item(self, *pane))
            })
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            identifiers()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            identifiers()
        }

        #[unsafe(method_id(toolbarSelectableItemIdentifiers:))]
        fn selectable(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            identifiers()
        }
    }

    impl Target {
        #[unsafe(method(termSettingsChanged:))]
        fn changed(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = tag_of(sender);
                with_open(|settings| settings.changed(tag));
            });
        }

        #[unsafe(method(termSettingsPane:))]
        fn pane(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = sender
                    .and_then(|sender| sender.downcast_ref::<NSToolbarItem>())
                    .map_or(0, |item| item.tag());
                if let Some(pane) = usize::try_from(tag).ok().and_then(|at| Pane::ALL.get(at)) {
                    with_open(|settings| settings.show_pane(*pane));
                }
            });
        }

        #[unsafe(method(termSettingsAddProfile:))]
        fn add_profile(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                with_open(|settings| settings.add_profile());
            });
        }

        #[unsafe(method(termSettingsDeleteProfile:))]
        fn delete_profile(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                with_open(|settings| settings.delete_profile());
            });
        }

        #[unsafe(method(termSettingsRecord:))]
        fn record(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = tag_of(sender);
                with_open(|settings| settings.record(tag));
            });
        }

        #[unsafe(method(termSettingsOpenFile:))]
        fn open_file(&self, _sender: Option<&AnyObject>) {
            guarded((), crate::open::settings);
        }
    }
);

fn tag_of(sender: Option<&AnyObject>) -> isize {
    sender
        .and_then(|sender| sender.downcast_ref::<NSControl>())
        .map_or(-1, |control| control.tag())
}

fn identifiers() -> Retained<NSArray<NSString>> {
    let ids: Vec<Retained<NSString>> = Pane::ALL.iter().map(|pane| term_ui::kit::ns(pane.id())).collect();
    NSArray::from_retained_slice(&ids)
}

impl Target {
    pub(super) fn new(mtm: MainThreadMarker) -> Retained<Target> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}
