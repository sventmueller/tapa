//! The sidebar's table: a click on the name of the selected workspace starts renaming it in place, as a click on the
//! name of the selected item does in Finder.

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSResponder, NSTableView};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSObjectProtocol, NSRect};
use term_ui::kit::guarded;

use crate::view::Sidebar;

/// The sidebar the table asks whether a click renames.
pub(crate) struct Ivars {
    sidebar: Weak<Sidebar>,
}

define_class!(
    // SAFETY: NSTableView has no subclassing requirements and `Table` does not implement Drop.
    #[unsafe(super(NSTableView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSidebarTable"]
    #[ivars = Ivars]
    pub(crate) struct Table;

    unsafe impl NSObjectProtocol for Table {}

    impl Table {
        #[unsafe(method(validateProposedFirstResponder:forEvent:))]
        fn validate_proposed_first_responder(&self, responder: &NSResponder, event: Option<&NSEvent>) -> bool {
            let renames = guarded(false, || {
                self.ivars()
                    .sidebar
                    .load()
                    .is_some_and(|sidebar| sidebar.rename_on_click(self, responder, || released_in_place(self, event)))
            });
            if renames {
                false
            } else {
                // SAFETY: NSTableView's own answer, with the arguments AppKit passed.
                unsafe { msg_send![super(self), validateProposedFirstResponder: responder, forEvent: event] }
            }
        }
    }
);

impl Table {
    /// The table of `sidebar`.
    pub(crate) fn make(mtm: MainThreadMarker, sidebar: &Sidebar) -> Retained<NSTableView> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            sidebar: Weak::from(sidebar),
        });
        // SAFETY: NSView's designated initializer, called once on the newly allocated view.
        let this: Retained<Table> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        Retained::into_super(this)
    }
}

/// Whether the left mouse button `event` pressed comes up without dragging, which the next mouse event tells; it stays
/// in the queue for the table, which starts a row's drag from it.
fn released_in_place(table: &NSTableView, event: Option<&NSEvent>) -> bool {
    if event.is_none_or(|event| event.r#type() != NSEventType::LeftMouseDown) {
        return true;
    }
    let Some(window) = table.window() else {
        return true;
    };
    // SAFETY: AppKit sets its exported run loop mode constants when it loads and never changes them.
    let mode = unsafe { NSDefaultRunLoopMode };
    let next = window.nextEventMatchingMask_untilDate_inMode_dequeue(
        NSEventMask::LeftMouseUp | NSEventMask::LeftMouseDragged,
        Some(&NSDate::distantFuture()),
        mode,
        false,
    );
    next.is_none_or(|next| next.r#type() == NSEventType::LeftMouseUp)
}
