//! macOS's microphone permission. Lowspeak's recorder only reports it as missing and never asks, and macOS lists an
//! app under Privacy & Security > Microphone only once it has asked; so the first dictation asks.

use std::sync::Mutex;
use term_base::LockIgnoringPoison;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

use crate::util::{guarded, main_soon};

/// Whether macOS has an answer about the microphone yet.
pub(super) fn undecided() -> bool {
    #[cfg(feature = "selftest")]
    if super::in_selftest() {
        return false;
    }
    macos_undecided()
}

/// Whether macOS itself has no answer about the microphone yet, whatever the self-test plays.
pub(super) fn macos_undecided() -> bool {
    // SAFETY: AVMediaTypeAudio is an immutable constant that AVFoundation sets when it loads.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    // SAFETY: `audio` is AVFoundation's own media type constant; the class method has no other precondition.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    status == AVAuthorizationStatus::NotDetermined
}

/// Shows macOS's question; `answered` runs on the main thread with whether the user allowed it.
pub(super) fn ask(answered: impl FnOnce(bool) + Send + 'static) {
    // SAFETY: AVMediaTypeAudio is an immutable constant that AVFoundation sets when it loads.
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return answered(false);
    };
    let answer = once_on_main(answered);
    let handler = RcBlock::new(move |granted: Bool| guarded((), || answer(granted)));
    // SAFETY: `audio` is AVFoundation's media type constant, and the handler, which AVFoundation calls once on a queue
    // of its own, only takes the callback out of its mutex and hands it to the main thread.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
}

/// `answered` as the handler AVFoundation calls on a queue of its own: the answer reaches the main thread, once.
pub(super) fn once_on_main(answered: impl FnOnce(bool) + Send + 'static) -> impl Fn(Bool) + Send + Sync + 'static {
    let answered = Mutex::new(Some(answered));
    move |granted: Bool| {
        let answered = answered.lock_ignoring_poison().take();
        if let Some(answered) = answered {
            let granted = granted.as_bool();
            main_soon(move || answered(granted));
        }
    }
}
