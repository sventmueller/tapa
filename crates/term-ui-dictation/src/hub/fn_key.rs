//! fn dictates as in Lowspeak, hold to talk or a double tap to lock, while the app is active and Lowspeak does not
//! run: Lowspeak owns fn while it runs, and an event tap takes fn from every app, so it lives only while this one is
//! in front. `Hotkey::stop` is final, so each of those times gets a new one.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::time::Instant;

use block2::RcBlock;
use lowspeak::app::gesture::{Command, FnGesture, SECOND_TAP_WINDOW};
use lowspeak::dictation::Target;
use lowspeak::hotkey::{Hotkey, Key as Pressed};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{NSApplication, NSRunningApplication, NSWorkspace};
use objc2_application_services::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{
    NSDictionary, NSNotification, NSNotificationCenter, NSNotificationName, NSNumber, NSObjectProtocol, NSString, NSURL,
};

use super::engine::LOWSPEAK_APP;
use super::in_selftest;
use crate::util::{constant, main_after, main_soon, ns};

/// The user default that says the app asked macOS for the Accessibility permission once; asking again is the user's
/// choice in the settings.
const ASKED: &str = "DictationAccessibilityAsked";

/// Where fn stands, for the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FnState {
    /// The setting is off.
    Off,
    /// The permission was never asked for; the first dictation does.
    NotAsked,
    /// Asked for and not given.
    Refused,
    /// Lowspeak runs and has fn.
    LowspeakHasIt,
    /// fn starts a dictation.
    Ready,
}

#[derive(Default)]
pub(super) struct FnKey {
    hotkey: Option<Hotkey>,
    gesture: FnGesture,
    /// The recording fn started.
    recording: Option<Target>,
    lowspeak_runs: bool,
    /// Asked in this run, so the tap waits for the permission instead of counting as refused.
    asked_now: bool,
    setting: Option<bool>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

fn with_fn<R>(f: impl FnOnce(&RefCell<FnKey>) -> R) -> R {
    f(&super::state().fn_key)
}

fn trusted() -> bool {
    // SAFETY: A plain query of the permission.
    unsafe { AXIsProcessTrusted() }
}

fn setting() -> bool {
    with_fn(|f| *f.borrow_mut().setting.get_or_insert_with(super::choices::with_fn))
}

pub(super) fn asked() -> bool {
    with_fn(|f| f.borrow().asked_now) || objc2_foundation::NSUserDefaults::standardUserDefaults().boolForKey(&ns(ASKED))
}

/// Calls `then` for each notification `name` of `center`, the app's own by default, for as long as the app runs. AppKit
/// and NSWorkspace post these on the main thread, and without a queue the call is synchronous, which quitting needs.
pub(super) fn observe(name: &NSNotificationName, center: Option<&NSNotificationCenter>, then: fn(&NSNotification)) {
    let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
        crate::util::guarded((), || {
            // SAFETY: The center passes a valid notification for the duration of the call.
            let notification = unsafe { notification.as_ref() };
            if MainThreadMarker::new().is_some() {
                then(notification);
            }
        });
    });
    let default = NSNotificationCenter::defaultCenter();
    let center = center.unwrap_or(&default);
    // SAFETY: The block runs on the posting thread and acts only there on the main thread, checked above.
    let observer = unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block) };
    with_fn(|f| f.borrow_mut().observers.push(observer));
}

pub(super) fn launched() {
    let lowspeak = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns(LOWSPEAK_APP));
    with_fn(|f| f.borrow_mut().lowspeak_runs = lowspeak.iter().any(|app| !app.isTerminated()));
    let workspace = NSWorkspace::sharedWorkspace().notificationCenter();
    observe(
        constant!(objc2_app_kit::NSWorkspaceDidLaunchApplicationNotification),
        Some(&workspace),
        |n| lowspeak_changed(n, true),
    );
    observe(
        constant!(objc2_app_kit::NSWorkspaceDidTerminateApplicationNotification),
        Some(&workspace),
        |n| lowspeak_changed(n, false),
    );
    observe(
        constant!(objc2_app_kit::NSApplicationDidBecomeActiveNotification),
        None,
        |_| arm(),
    );
    observe(
        constant!(objc2_app_kit::NSApplicationDidResignActiveNotification),
        None,
        |_| {
            if let Some(target) = with_fn(|f| f.borrow_mut().recording.take()) {
                super::end_with_fn(target);
            }
            arm();
        },
    );
    arm();
}

fn lowspeak_changed(notification: &NSNotification, runs: bool) {
    let app = notification
        .userInfo()
        .and_then(|info| info.objectForKey(constant!(objc2_app_kit::NSWorkspaceApplicationKey)))
        .and_then(|app| app.downcast::<NSRunningApplication>().ok());
    if app
        .and_then(|app| app.bundleIdentifier())
        .is_some_and(|id| id.to_string() == LOWSPEAK_APP)
    {
        with_fn(|f| f.borrow_mut().lowspeak_runs = runs);
        arm();
    }
}

/// Where fn stands now.
pub fn fn_state() -> FnState {
    if !setting() {
        FnState::Off
    } else if with_fn(|f| f.borrow().lowspeak_runs) {
        FnState::LowspeakHasIt
    } else if trusted() {
        FnState::Ready
    } else if asked() {
        FnState::Refused
    } else {
        FnState::NotAsked
    }
}

/// Starts the event tap when fn should dictate and stops it when not.
fn arm() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let wanted = !in_selftest()
        && setting()
        && NSApplication::sharedApplication(mtm).isActive()
        && !with_fn(|f| f.borrow().lowspeak_runs)
        && (trusted() || with_fn(|f| f.borrow().asked_now));
    let stopped = with_fn(|f| {
        let mut f = f.borrow_mut();
        match (&f.hotkey, wanted) {
            (None, true) => {
                f.gesture.reset();
                f.hotkey = Some(Hotkey::start(|key| main_soon(move || pressed(key))));
                None
            }
            (Some(_), false) => {
                f.gesture.reset();
                f.hotkey.take()
            }
            _ => None,
        }
    });
    if let Some(hotkey) = stopped {
        hotkey.stop();
    }
}

/// The user switched fn in the settings.
pub fn setting_changed(on: bool) {
    with_fn(|f| f.borrow_mut().setting = Some(on));
    arm();
}

pub(super) fn pressed(key: Pressed) {
    let now = Instant::now();
    let command = with_fn(|f| {
        let mut f = f.borrow_mut();
        match key {
            Pressed::FnDown { shift } => f.gesture.down(now, shift),
            Pressed::FnUp => f.gesture.up(now),
            Pressed::ShiftDown => f.gesture.shift_down(),
        }
    });
    obey(command);
}

fn obey(command: Command) {
    match command {
        Command::Start | Command::StartHandsFree => start(),
        Command::Stop => {
            if let Some(target) = with_fn(|f| f.borrow_mut().recording.take()) {
                super::end_with_fn(target);
            }
        }
        Command::AwaitSecondTap(tap) => main_after(SECOND_TAP_WINDOW, move || {
            obey(with_fn(|f| f.borrow_mut().gesture.second_tap_missed(tap)));
        }),
        Command::Lock | Command::None => {}
    }
}

/// fn went down: a dictation into the field with the focus; elsewhere, such as in a terminal, fn does nothing.
fn start() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let started = super::focused(mtm).filter(|target| super::begin_with_fn(*target));
    with_fn(|f| {
        let mut f = f.borrow_mut();
        f.recording = started;
        if started.is_none() {
            f.gesture.reset();
            if let Some(hotkey) = &f.hotkey {
                hotkey.reset_fn_state();
            }
        }
    });
}

/// The recording of `target` ended some other way, such as its limit or a click on its button.
pub(super) fn ended(target: Target) {
    with_fn(|f| {
        let mut f = f.borrow_mut();
        if f.recording == Some(target) {
            f.recording = None;
            f.gesture.reset();
        }
    });
}

/// After the first dictation the app asks for the Accessibility permission that fn needs (E13), once.
pub(super) fn after_dictation() {
    if in_selftest() || !setting() || trusted() || asked() {
        return;
    }
    request(false);
}

/// The settings ask again: macOS may not show its question a second time, so its pane opens as well.
pub fn ask_permission() {
    request(true);
}

fn request(open_pane: bool) {
    objc2_foundation::NSUserDefaults::standardUserDefaults().setBool_forKey(true, &ns(ASKED));
    with_fn(|f| f.borrow_mut().asked_now = true);
    if in_selftest() {
        return;
    }
    prompt();
    if open_pane
        && let Some(url) = NSURL::URLWithString(&ns(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        ))
    {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
    arm();
}

fn prompt() {
    // SAFETY: A framework constant; CFString and NSString are toll-free bridged.
    let key: &NSString = unsafe { &*std::ptr::from_ref(kAXTrustedCheckOptionPrompt).cast::<NSString>() };
    let yes = NSNumber::new_bool(true);
    let value: &AnyObject = &yes;
    let options = NSDictionary::from_slices(&[key], &[value]);
    // SAFETY: NSDictionary and CFDictionary are toll-free bridged.
    let options: &CFDictionary = unsafe { &*(Retained::as_ptr(&options).cast::<CFDictionary>()) };
    // SAFETY: The options hold the documented key with a boolean value.
    unsafe { AXIsProcessTrustedWithOptions(Some(options)) };
}
