//! What the view asks of AppKit for itself: calls on a later turn of the run loop.

use objc2::runtime::Sel;

use super::EditorTextView;

impl EditorTextView {
    /// Runs `selector` on this view after `delay` seconds, replacing a request for it that has not run yet.
    pub(crate) fn later(&self, selector: Sel, delay: f64) {
        use objc2_foundation::NSObjectNSDelayedPerforming;
        let object: &objc2::runtime::NSObject = self;
        // SAFETY: A selector without arguments that this class answers; the run loop retains the target until the call.
        unsafe {
            objc2::runtime::NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(object, selector, None);
            let modes = objc2_foundation::NSArray::from_slice(&[objc2_foundation::NSRunLoopCommonModes]);
            object.performSelector_withObject_afterDelay_inModes(selector, None, delay, &modes);
        }
    }
}

/// Runs `f` as the interval `name` on Instruments' Points of Interest track (SN-M1).
pub(crate) fn step(name: term_base::signpost::Name, f: impl FnOnce()) {
    let _interval = term_base::signpost::begin(name);
    f();
}
