//! The pointer over a file row of the folder tree reads that file ahead, so that clicking it opens without a wait.

use std::cell::RefCell;

use crate::prewarm::Prewarm;
use objc2::rc::{Retained, Weak};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSOutlineView, NSTrackingArea, NSTrackingAreaOptions};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize};

use super::view::path_of;
use term_ui::kit::guarded;

pub(crate) struct HoverIvars {
    outline: Weak<NSOutlineView>,
    area: RefCell<Option<Retained<NSTrackingArea>>>,
    prewarm: Prewarm,
}

impl Drop for HoverIvars {
    fn drop(&mut self) {
        // The area holds its owner unretained, so it must not outlive this object.
        if let (Some(outline), Some(area)) = (self.outline.load(), self.area.get_mut().take()) {
            outline.removeTrackingArea(&area);
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars' Drop only removes the tracking area.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermFilesFileTreeHover"]
    #[ivars = HoverIvars]
    pub(crate) struct TreeHover;

    unsafe impl NSObjectProtocol for TreeHover {}

    impl TreeHover {
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            guarded((), || self.moved(event));
        }
    }
);

impl TreeHover {
    /// Follows the pointer over `outline`.
    pub(crate) fn new(mtm: MainThreadMarker, outline: &NSOutlineView) -> Retained<TreeHover> {
        let this = TreeHover::alloc(mtm).set_ivars(HoverIvars {
            outline: Weak::from(outline),
            area: RefCell::new(None),
            prewarm: Prewarm::default(),
        });
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        let this: Retained<TreeHover> = unsafe { msg_send![super(this), init] };
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: The hover removes the area when it goes, so the area never calls an owner that is gone.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                options,
                Some(&this),
                None,
            )
        };
        outline.addTrackingArea(&area);
        *this.ivars().area.borrow_mut() = Some(area);
        this
    }

    fn moved(&self, event: &NSEvent) {
        let Some(outline) = self.ivars().outline.load() else {
            return;
        };
        let point = outline.convertPoint_fromView(event.locationInWindow(), None);
        let row = outline.rowAtPoint(point);
        let Some(item) = (row >= 0).then(|| outline.itemAtRow(row)).flatten() else {
            return;
        };
        // SAFETY: The item came from this outline view, which knows it.
        if unsafe { outline.isExpandable(Some(&item)) } {
            return;
        }
        if let Some(path) = path_of(&item) {
            self.ivars().prewarm.file(&path);
        }
    }
}
