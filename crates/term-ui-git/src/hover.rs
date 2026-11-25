//! Rows that do something while the pointer is over them: buttons that show, built on the first hover; a tooltip
//! written only when AppKit shows it; and a read that waits until the pointer rests and the list stands still.

use std::cell::{Cell, OnceCell};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSEvent, NSLayoutConstraint, NSTableCellView, NSToolTipTag, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSViewToolTipOwner,
};
use objc2_foundation::{NSObjectNSDelayedPerforming, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use term_ui::kit::{center_y, guarded, ns};

/// How long the pointer stays on a row before the row reads what it tells on hover.
const REST: Duration = Duration::from_millis(350);

/// The buttons a row shows on hover, before they are built.
struct Actions {
    build: Box<dyn Fn() -> Option<Retained<NSView>>>,
    /// The text that keeps clear of the buttons.
    text: Retained<NSView>,
    /// The view the buttons sit before.
    before: Retained<NSView>,
    gap: f64,
    /// Keeps the text clear of `before` while the buttons do not show.
    to_edge: Retained<NSLayoutConstraint>,
}

#[derive(Default)]
pub(crate) struct HoverIvars {
    /// What runs once the pointer rested on the row.
    rest: OnceCell<Box<dyn Fn()>>,
    /// Where the list was scrolled when the wait began.
    origin: Cell<NSPoint>,
    tip: OnceCell<Box<dyn Fn() -> Option<String>>>,
    actions: OnceCell<Actions>,
    /// The built buttons, and the constraint that keeps the text clear of them while they show.
    shown: OnceCell<(Retained<NSView>, Retained<NSLayoutConstraint>)>,
}

define_class!(
    // SAFETY: NSTableCellView has no subclassing requirements and `HoverCell` does not implement Drop.
    /// A row whose buttons show while the mouse is over it.
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitHoverCell"]
    #[ivars = HoverIvars]
    pub(crate) struct HoverCell;

    unsafe impl NSObjectProtocol for HoverCell {}

    unsafe impl NSViewToolTipOwner for HoverCell {
        // SAFETY: The protocol's method with its signature; the body reads neither the view nor the raw user data.
        #[unsafe(method_id(view:stringForToolTip:point:userData:))]
        unsafe fn tool_tip(
            &self,
            _view: &NSView,
            _tag: NSToolTipTag,
            _point: NSPoint,
            _data: *mut std::ffi::c_void,
        ) -> Retained<NSString> {
            let text = guarded(None, || self.ivars().tip.get().and_then(|tip| tip()));
            ns(&text.unwrap_or_default())
        }
    }

    impl HoverCell {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            guarded((), || {
                self.show_actions(true);
                self.begin_rest();
            });
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            guarded((), || {
                self.show_actions(false);
                self.end_rest();
            });
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            // SAFETY: The overridden method, which takes no arguments.
            let _: () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
            guarded((), || {
                self.show_actions(false);
                self.end_rest();
            });
        }

        #[unsafe(method(termGitWaited))]
        fn waited_action(&self) {
            guarded((), || self.waited());
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            guarded((), || self.offer_tip());
        }
    }
);

impl HoverCell {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<HoverCell> {
        let this = HoverCell::alloc(mtm).set_ivars(HoverIvars::default());
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(200.0, term_ui::kit::style::ROW_HEIGHT),
        );
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let cell: Retained<HoverCell> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: The cell owns the area it adds to itself; AppKit keeps the owner unretained, and the area goes with
        // the cell.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&cell),
                None,
            )
        };
        cell.addTrackingArea(&area);
        cell
    }

    /// Runs `rested` once the pointer stayed on the row for a moment while the list stood still.
    pub(crate) fn on_rest(&self, rested: impl Fn() + 'static) {
        let _ = self.ivars().rest.set(Box::new(rested));
    }

    /// Shows what `tip` gives as the row's tooltip, asked for only when the tooltip comes up.
    pub(crate) fn set_tip(&self, tip: impl Fn() -> Option<String> + 'static) {
        let _ = self.ivars().tip.set(Box::new(tip));
        self.offer_tip();
    }

    /// The buttons that `build` makes on the first hover, before `before`, with `gap` to it and to `text`; while they
    /// do not show, `text` keeps `edge_gap` from `before`.
    pub(crate) fn set_actions(
        &self,
        build: impl Fn() -> Option<Retained<NSView>> + 'static,
        text: &NSView,
        before: &NSView,
        gap: f64,
        edge_gap: f64,
    ) {
        let to_edge = text
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&before.leadingAnchor(), -edge_gap);
        to_edge.setActive(true);
        let _ = self.ivars().actions.set(Actions {
            build: Box::new(build),
            text: text.retain(),
            before: before.retain(),
            gap,
            to_edge,
        });
    }

    fn offer_tip(&self) {
        if self.ivars().tip.get().is_none() {
            return;
        }
        self.removeAllToolTips();
        // SAFETY: The cell owns its tooltip area and answers it as its owner; no user data is passed.
        unsafe { self.addToolTipRect_owner_userData(self.bounds(), self, std::ptr::null_mut()) };
    }

    fn show_actions(&self, visible: bool) {
        let ivars = self.ivars();
        let Some(actions) = ivars.actions.get() else { return };
        if visible && ivars.shown.get().is_none() {
            let Some(view) = (actions.build)() else { return };
            self.addSubview(&view);
            center_y(&view, self);
            view.trailingAnchor()
                .constraintEqualToAnchor_constant(&actions.before.leadingAnchor(), -actions.gap)
                .setActive(true);
            let beside = actions
                .text
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.leadingAnchor(), -actions.gap);
            let _ = ivars.shown.set((view, beside));
        }
        if let Some((view, beside)) = ivars.shown.get() {
            view.setHidden(!visible);
            actions.to_edge.setActive(!visible);
            beside.setActive(visible);
        }
    }

    fn begin_rest(&self) {
        let ivars = self.ivars();
        if ivars.rest.get().is_none() {
            return;
        }
        ivars.origin.set(self.scrolled_to());
        self.wait();
    }

    fn end_rest(&self) {
        let object: &AnyObject = self;
        // SAFETY: Cancels only this cell's own wait, whose selector takes no argument.
        unsafe { NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(object, sel!(termGitWaited), None) };
    }

    /// Waits for the pointer to rest; in the default run loop mode only, so not while a scroller is dragged.
    fn wait(&self) {
        self.end_rest();
        let object: &NSObject = self;
        // SAFETY: A selector without arguments that this class answers; the run loop retains the cell until the call.
        unsafe { object.performSelector_withObject_afterDelay(sel!(termGitWaited), None, REST.as_secs_f64()) };
    }

    /// The wait is over: the row reads when the pointer is still on it and the list did not scroll meanwhile; a list
    /// that scrolled waits again.
    fn waited(&self) {
        let ivars = self.ivars();
        if !self.under_pointer() {
            return;
        }
        let origin = self.scrolled_to();
        if origin != ivars.origin.get() || NSEvent::pressedMouseButtons() != 0 {
            ivars.origin.set(origin);
            return self.wait();
        }
        if let Some(rested) = ivars.rest.get() {
            rested();
        }
    }

    fn scrolled_to(&self) -> NSPoint {
        self.enclosingScrollView()
            .map_or(NSPoint::ZERO, |scroll| scroll.contentView().bounds().origin)
    }

    fn under_pointer(&self) -> bool {
        let Some(window) = self.window() else { return false };
        let point = self.convertPoint_fromView(window.mouseLocationOutsideOfEventStream(), None);
        self.mouse_inRect(point, self.bounds())
    }
}
