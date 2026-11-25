//! A view over the text for what TextKit does not draw: the placeholders of folds and the carets and selections beyond
//! the first. Clicks pass through to the text.

use std::cell::Cell;

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAffineTransformNSAppKitAdditions, NSBezierPath, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSStringDrawing, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::view::EditorTextView;
use crate::util::{constant, guarded, ns};

pub(crate) struct OverlayIvars {
    editor: Weak<EditorTextView>,
    /// The top left of the part on screen when it last drew.
    drawn_at: Cell<Option<NSPoint>>,
    /// Whether it waits to draw again because it drew over lines a scroll just brought on screen.
    again: Cell<bool>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Overlay` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermEditorOverlay"]
    #[ivars = OverlayIvars]
    pub(crate) struct Overlay;

    unsafe impl NSObjectProtocol for Overlay {}

    impl Overlay {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
            None
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }

        #[unsafe(method(termDrawAgain))]
        fn draw_again(&self) {
            self.ivars().again.set(false);
            self.setNeedsDisplay(true);
        }
    }
);

/// Puts an overlay over the part of `editor` on screen. It has a layer of its own: without one it would draw into the
/// text view's layer, under the layers TextKit 2 draws the text in. Covering only what is on screen keeps that layer
/// small in a long text.
pub(crate) fn attach(editor: &EditorTextView) {
    let this = Overlay::alloc(editor.mtm()).set_ivars(OverlayIvars {
        editor: Weak::from(editor),
        drawn_at: Cell::new(None),
        again: Cell::new(false),
    });
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let this: Retained<Overlay> = unsafe { msg_send![super(this), initWithFrame: editor.visibleRect()] };
    this.setWantsLayer(true);
    this.setAccessibilityElement(false);
    editor.addSubview_positioned_relativeTo(&this, NSWindowOrderingMode::Above, None);
    *editor.ivars().around.overlay.borrow_mut() = Some(Weak::from(&*this));
}

/// How often TextKit told an editor it laid out the lines on screen, for the self-test.
#[cfg(feature = "selftest")]
pub(crate) static LAID_OUT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// TextKit laid out the lines on screen: the overlay moves over them and draws again, as what it draws follows the
/// layout.
pub(crate) fn laid_out(editor: &EditorTextView) {
    #[cfg(feature = "selftest")]
    LAID_OUT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Some(overlay) = editor.ivars().around.overlay.borrow().as_ref().and_then(Weak::load) {
        let subviews = editor.subviews();
        let on_top = subviews
            .lastObject()
            .is_some_and(|top| std::ptr::eq(&*top, &**overlay as &NSView));
        // TextKit may put views of its own over the overlay as it lays out, which would hide what the overlay draws.
        if !on_top {
            editor.addSubview_positioned_relativeTo(&overlay, NSWindowOrderingMode::Above, None);
        }
        // A layout after the overlay drew places the lines it drew over, and `follow` draws it from there.
        if overlay.ivars().again.replace(false) {
            overlay.cancel_draw_again();
        }
    }
    follow(editor);
}

/// Moves the overlay of `editor` over the part on screen and draws it there.
pub(crate) fn follow(editor: &EditorTextView) {
    if let Some(overlay) = editor.ivars().around.overlay.borrow().as_ref().and_then(Weak::load) {
        let seen = editor.visibleRect();
        if overlay.frame() != seen {
            overlay.setFrame(seen);
        }
        overlay.setNeedsDisplay(true);
    }
}

/// Draws the overlay of `editor` again.
pub(crate) fn redraw(editor: &EditorTextView) {
    if let Some(overlay) = editor.ivars().around.overlay.borrow().as_ref().and_then(Weak::load) {
        overlay.setNeedsDisplay(true);
    }
}

impl Overlay {
    fn draw(&self) {
        let Some(editor) = self.ivars().editor.load() else {
            return;
        };
        let seen = editor.visibleRect().origin;
        if self.ivars().drawn_at.replace(Some(seen)) != Some(seen) {
            self.draw_again_soon();
        }
        // What follows draws in the editor's coordinates, of which the overlay shows the part on screen.
        let origin = self.frame().origin;
        let shift = objc2_foundation::NSAffineTransform::transform();
        shift.translateXBy_yBy(-origin.x, -origin.y);
        shift.concat();
        self.draw_placeholders(&editor);
        self.draw_carets(&editor);
        super::vim::draw_block(&editor);
    }

    /// Draws again on the next turn of the run loop, unless TextKit lays the lines out before (`laid_out`). While the
    /// overlay draws over lines a scroll just brought on screen, TextKit 2 answers their places with the bottom of
    /// what shows; it knows them once the text is drawn.
    fn draw_again_soon(&self) {
        use objc2_foundation::NSObjectNSDelayedPerforming;
        self.ivars().again.set(true);
        let object: &objc2::runtime::NSObject = self;
        // SAFETY: A selector without arguments that this class answers; the run loop retains the target until the call.
        unsafe {
            let modes = objc2_foundation::NSArray::from_slice(&[objc2_foundation::NSRunLoopCommonModes]);
            object.performSelector_withObject_afterDelay_inModes(objc2::sel!(termDrawAgain), None, 0.0, &modes);
        }
    }

    fn cancel_draw_again(&self) {
        use objc2_foundation::NSObjectNSDelayedPerforming;
        let object: &objc2::runtime::NSObject = self;
        // SAFETY: Cancels the request `draw_again_soon` made with this target, selector and no argument.
        unsafe {
            objc2::runtime::NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(
                object,
                objc2::sel!(termDrawAgain),
                None,
            );
        }
    }

    /// A rounded box with an ellipsis after the first line of every fold on screen.
    fn draw_placeholders(&self, editor: &EditorTextView) {
        let size = editor.ivars().font_size.get() * 0.9;
        let attributes = NSDictionary::<NSString, objc2::runtime::AnyObject>::from_retained_objects(
            &[
                constant!(NSFontAttributeName),
                constant!(NSForegroundColorAttributeName),
            ],
            &[
                NSFont::systemFontOfSize(size).into(),
                NSColor::secondaryLabelColor().into(),
            ],
        );
        for (rect, _) in super::fold::placeholders(editor) {
            NSColor::quaternaryLabelColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 4.0, 4.0).fill();
            let dots = ns("…");
            // SAFETY: The attributes hold a font and a colour, as the method expects.
            let width = unsafe { dots.sizeWithAttributes(Some(&attributes)) }.width;
            let at = NSPoint::new(rect.origin.x + (rect.size.width - width) / 2.0, rect.origin.y - 1.0);
            // SAFETY: These attributes too hold only a font and a colour under AppKit's keys.
            unsafe { dots.drawAtPoint_withAttributes(at, Some(&attributes)) };
        }
    }

    /// The selections beyond the first as tinted areas, and their carets as lines.
    fn draw_carets(&self, editor: &EditorTextView) {
        let mut carets = super::multi::extra(editor);
        carets.retain(|range| !super::fold::hidden_by_fold(editor, range.end));
        if carets.is_empty() {
            return;
        }
        for range in &carets {
            if range.is_empty() {
                continue;
            }
            NSColor::selectedTextBackgroundColor()
                .colorWithAlphaComponent(0.4)
                .setFill();
            for rect in super::multi::rects(editor, range.clone()) {
                NSBezierPath::fillRect(rect);
            }
        }
        editor.insertionPointColor().setFill();
        for range in &carets {
            if let Some(rect) = crate::text_view::rect_of(editor, range.end..range.end) {
                NSBezierPath::fillRect(NSRect::new(rect.origin, NSSize::new(1.5, rect.size.height)));
            }
        }
    }
}
