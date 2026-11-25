use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol};

use super::guarded;

/// A watch on the default notification center that ends when the value drops, so it lives exactly as long as what
/// keeps it.
#[must_use = "the watch ends when this drops"]
pub struct Observation {
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Drop for Observation {
    fn drop(&mut self) {
        // SAFETY: The token came from this center's block-based `addObserverForName` and is removed only here, once.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.token.as_ref()) };
    }
}

/// Runs `f`, guarded, for each notification `name` that `object` posts on the main thread, or any sender when
/// `None`; the watch ends when the returned value drops.
///
/// `f` runs synchronously inside `postNotification`, so a view that follows a scroll or resize redraws in the same
/// pass instead of one run loop turn late, as it would through the main operation queue. A post from another thread
/// does not reach `f`.
pub fn observe(
    mtm: MainThreadMarker,
    name: &NSNotificationName,
    object: Option<&AnyObject>,
    f: impl Fn(&NSNotification) + 'static,
) -> Observation {
    let f = MainThreadBound::new(f, mtm);
    let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
        guarded((), || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            // SAFETY: The center hands the block a notification it keeps alive for the length of the call.
            let notification = unsafe { notification.as_ref() };
            f.get(mtm)(notification);
        });
    });
    // SAFETY: The block captures only a `MainThreadBound`, which may be called and released on any thread and lets
    // `f` run only on the main thread; without a queue the center calls the block on the posting thread.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            object,
            None,
            &block,
        )
    };
    Observation { token }
}
