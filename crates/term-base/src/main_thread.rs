//! The main thread: running callbacks from the system without unwinding through its frames, and hopping onto it.

use crate::lane::contained;

/// What a callback from the system does, with `fallback` if a bug in it panics; a panic would otherwise unwind through
/// the system's frames.
pub fn guarded<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    contained(f).unwrap_or(fallback)
}

/// Runs `f` on the main thread when its turn comes. dispatch2 calls it through `extern "C"`, so a panic that left it
/// would end the app.
pub fn main_soon(f: impl FnOnce() + Send + 'static) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        contained(f);
    });
}

/// `main_soon` after `delay`.
pub fn main_after(delay: std::time::Duration, f: impl FnOnce() + Send + 'static) {
    if let Ok(when) = dispatch2::DispatchTime::try_from(delay) {
        let _ = dispatch2::DispatchQueue::main().after(when, move || {
            contained(f);
        });
    }
}

/// Runs `f` on the main thread from the run loop instead of the main dispatch queue, for work that may show a modal
/// alert: the queue runs one callback at a time, so a modal session inside one holds up every other until it ends.
pub fn main_on_run_loop(f: impl FnOnce() + Send + 'static) {
    let f = std::cell::Cell::new(Some(f));
    let block = block2::RcBlock::new(move || {
        contained(|| {
            if let Some(f) = f.take() {
                f();
            }
        });
    });
    // SAFETY: The block holds only what may go to another thread, and the main run loop calls it on the main thread.
    unsafe { objc2_foundation::NSRunLoop::mainRunLoop().performBlock(&block) };
    // A block queued on a run loop does not wake it, so an idle main thread would hold it until the next event.
    if let Some(main) = objc2_core_foundation::CFRunLoop::main() {
        main.wake_up();
    }
}

/// Core Animation commits a frame from a run loop observer of this order before the main thread sleeps.
const CORE_ANIMATION_COMMIT: isize = 2_000_000;

/// Runs `f` on the main thread once the run loop has committed the next frame, such as the first one of a window that
/// just opened; at once where the run loop cannot be observed.
pub fn after_frame(f: impl FnOnce() + 'static) {
    let f = std::rc::Rc::new(std::cell::Cell::new(Some(f)));
    let pending = std::rc::Rc::clone(&f);
    let block = block2::RcBlock::new(
        move |_: *mut objc2_core_foundation::CFRunLoopObserver, _: objc2_core_foundation::CFRunLoopActivity| {
            guarded((), || {
                if let Some(f) = pending.take() {
                    f();
                }
            })
        },
    );
    let main = objc2_core_foundation::CFRunLoop::main();
    // SAFETY: The default allocator and a block the main run loop calls on the main thread, once, as it does not
    // repeat.
    let observer = unsafe {
        objc2_core_foundation::CFRunLoopObserver::with_handler(
            None,
            objc2_core_foundation::CFRunLoopActivity::BeforeWaiting.0,
            false,
            CORE_ANIMATION_COMMIT + 1,
            Some(&block),
        )
    };
    match (main, observer) {
        (Some(main), Some(observer)) => {
            // SAFETY: Reading a constant CoreFoundation exports.
            let common = unsafe { objc2_core_foundation::kCFRunLoopCommonModes };
            main.add_observer(Some(&observer), common);
        }
        _ => {
            if let Some(f) = f.take() {
                f();
            }
        }
    }
}
