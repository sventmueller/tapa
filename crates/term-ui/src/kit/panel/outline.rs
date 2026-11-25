//! A panel's outline view whose disclosure triangles begin where the panel's title does, as in the other panels.

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSOutlineView;
use objc2_foundation::{NSObjectProtocol, NSRect};

use super::{LIST_FRAME, OUTDENT, outdent};

/// Where the rows below the top level of a [`PanelOutline`] have their triangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nesting {
    /// One indentation further in per level, as AppKit puts them.
    Indented,
    /// Where the top level's triangle is, so a section's rows begin where its title does.
    UnderTitle,
}

define_class!(
    // SAFETY: NSOutlineView has no subclassing requirements and `PanelOutline` does not implement Drop.
    /// A panel's outline view, its disclosure triangles where the panel's title begins.
    #[unsafe(super(NSOutlineView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermPanelOutlineView"]
    #[ivars = Nesting]
    pub struct PanelOutline;

    unsafe impl NSObjectProtocol for PanelOutline {}

    impl PanelOutline {
        /// AppKit makes no disclosure button, whose observers would keep it for good; the row's own chevron stands
        /// where it would be.
        #[unsafe(method(frameOfOutlineCellAtRow:))]
        fn no_outline_cell(&self, _row: isize) -> NSRect {
            NSRect::ZERO
        }

        #[unsafe(method(didAddRowView:forRow:))]
        fn did_add_row_view(&self, row_view: &objc2_app_kit::NSTableRowView, row: isize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), didAddRowView: row_view, forRow: row] };
            crate::kit::guarded((), || {
                // SAFETY: This class's own method, with its signature.
                let frame: NSRect = unsafe { msg_send![self, termDisclosureFrameAtRow: row] };
                crate::kit::panel::chevron::place(self, row_view, frame);
            });
        }

        #[unsafe(method(termDisclosureFrameAtRow:))]
        fn outline_cell_frame(&self, row: isize) -> NSRect {
            // SAFETY: The superclass's implementation, with this method's signature.
            let frame: NSRect = unsafe { msg_send![super(self), frameOfOutlineCellAtRow: row] };
            crate::kit::guarded(frame, || self.toward_title(frame, row, false))
        }

        #[unsafe(method(frameOfCellAtColumn:row:))]
        fn cell_frame(&self, column: isize, row: isize) -> NSRect {
            // SAFETY: The superclass's implementation, with this method's signature.
            let frame: NSRect = unsafe { msg_send![super(self), frameOfCellAtColumn: column, row: row] };
            if column != 0 {
                return frame;
            }
            crate::kit::guarded(frame, || self.toward_title(frame, row, true))
        }
    }
);

impl PanelOutline {
    /// An outline view of [`LIST_FRAME`] whose rows below the top level nest as `nesting` says.
    pub fn new(mtm: MainThreadMarker, nesting: Nesting) -> Retained<PanelOutline> {
        let this = PanelOutline::alloc(mtm).set_ivars(nesting);
        // SAFETY: NSOutlineView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), initWithFrame: LIST_FRAME] }
    }

    /// `frame` of the triangle or, when `cell`, of the first column's cell of `row`, moved toward the panel's title.
    fn toward_title(&self, frame: NSRect, row: isize, cell: bool) -> NSRect {
        let frame = outdent(self, frame, OUTDENT, cell);
        if *self.ivars() == Nesting::UnderTitle && self.levelForRow(row) > 0 {
            outdent(self, frame, self.indentationPerLevel(), cell)
        } else {
            frame
        }
    }
}
