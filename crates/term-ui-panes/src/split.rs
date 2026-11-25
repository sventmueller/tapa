//! A split of the tree as an `NSSplitView`: its parts at their shares, and a delegate that writes the shares back
//! when the user drags a divider.

use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSSplitView, NSSplitViewDelegate, NSSplitViewDividerStyle, NSView};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSRect};
use term_panes::SplitId;

use crate::Inner;

/// The least a pane keeps beside a divider the user drags, in points.
const LEAST: f64 = 80.0;

/// What a split's delegate keeps.
pub(crate) struct State {
    split: SplitId,
    side_by_side: bool,
    shares: Vec<f64>,
    panes: Weak<Inner>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSplitDelegate"]
    #[ivars = std::cell::RefCell<State>]
    pub(crate) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSSplitViewDelegate for Delegate {
        #[unsafe(method(splitViewDidResizeSubviews:))]
        fn did_resize(&self, notification: &NSNotification) {
            term_base::guarded((), || self.resized(notification));
        }

        #[unsafe(method(splitView:constrainMinCoordinate:ofSubviewAt:))]
        fn constrain_min(&self, _view: &NSSplitView, proposed: f64, _index: isize) -> f64 {
            proposed + LEAST
        }

        #[unsafe(method(splitView:constrainMaxCoordinate:ofSubviewAt:))]
        fn constrain_max(&self, _view: &NSSplitView, proposed: f64, _index: isize) -> f64 {
            proposed - LEAST
        }
    }
);

impl Delegate {
    fn resized(&self, notification: &NSNotification) {
        let Some(view) = notification
            .object()
            .and_then(|object| object.downcast::<NSSplitView>().ok())
        else {
            return;
        };
        let mut state = self.ivars().borrow_mut();
        let Some(panes) = state.panes.upgrade() else {
            return;
        };
        if panes.building.get() {
            return;
        }
        let lengths: Vec<f64> = view
            .subviews()
            .iter()
            .map(|child| {
                let size = child.frame().size;
                if state.side_by_side { size.width } else { size.height }
            })
            .collect();
        let total: f64 = lengths.iter().sum();
        if total <= 0.0 || lengths.len() != state.shares.len() {
            return;
        }
        let shares: Vec<f64> = lengths.iter().map(|length| length / total).collect();
        state.shares.clone_from(&shares);
        panes.tree.borrow_mut().set_shares(state.split, &shares);
    }
}

/// A split view of `children` side by side or one above the other, with its delegate, which the caller keeps alive.
pub(crate) fn make(
    mtm: MainThreadMarker,
    split: SplitId,
    side_by_side: bool,
    shares: &[f64],
    children: &[Retained<NSView>],
    panes: &Rc<Inner>,
) -> (Retained<NSView>, Retained<Delegate>) {
    let view = NSSplitView::new(mtm);
    view.setVertical(side_by_side);
    view.setDividerStyle(NSSplitViewDividerStyle::Thin);
    view.setFrame(NSRect::new(
        objc2_foundation::NSPoint::ZERO,
        objc2_foundation::NSSize::new(1000.0, 1000.0),
    ));
    for child in children {
        view.addSubview(child);
    }
    let delegate = Delegate::alloc(mtm).set_ivars(std::cell::RefCell::new(State {
        split,
        side_by_side,
        shares: shares.to_vec(),
        panes: Rc::downgrade(panes),
    }));
    // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
    let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
    view.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    (Retained::into_super(view), delegate)
}

/// Lays out `view` and the split views inside it at the shares their delegates hold.
pub(crate) fn apply_shares(view: &NSView) {
    let Some(split) = view.downcast_ref::<NSSplitView>() else {
        return;
    };
    split.adjustSubviews();
    let shares = split
        .delegate()
        .and_then(|delegate| {
            let object: &objc2::runtime::AnyObject = delegate.as_ref();
            object
                .downcast_ref::<Delegate>()
                .map(|d| d.ivars().borrow().shares.clone())
        })
        .unwrap_or_default();
    let size = split.bounds().size;
    let length = if split.isVertical() { size.width } else { size.height };
    let children = split.subviews();
    let dividers = split.dividerThickness() * (children.len().saturating_sub(1) as f64);
    let room = (length - dividers).max(0.0);
    let mut position = 0.0;
    for (index, share) in shares.iter().enumerate().take(children.len().saturating_sub(1)) {
        position += share * room;
        let index = isize::try_from(index).unwrap_or(0);
        split.setPosition_ofDividerAtIndex(position, index);
        position += split.dividerThickness();
    }
    for child in children.iter() {
        apply_shares(&child);
    }
}
