//! The global hotkey through Carbon's `RegisterEventHotKey`, which, unlike watching every key press, needs no
//! accessibility permission. A chord's character is found on the keyboard layout in use, so `ctrl+ö` works on a
//! German keyboard; named keys have fixed key codes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use objc2_core_graphics::CGEvent;
use term_config::Chord;

type OSStatus = i32;
type EventRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventHandlerUPP = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const SIGNATURE: u32 = u32::from_be_bytes(*b"quik");
const CMD_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;
const NO_ERR: OSStatus = 0;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerUPP,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> OSStatus;
}

thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    static REGISTERED: RefCell<Vec<EventHotKeyRef>> = const { RefCell::new(Vec::new()) };
}

/// Makes `chords` the hotkeys, in place of those before.
pub(super) fn register(chords: &[Chord]) {
    install_handler();
    REGISTERED.with(|registered| {
        for hot_key in registered.borrow_mut().drain(..) {
            // SAFETY: Each was registered below and is unregistered once.
            unsafe { UnregisterEventHotKey(hot_key) };
        }
    });
    for (index, chord) in chords.iter().enumerate() {
        let Some(code) = key_code(&chord.key) else {
            term_base::diag::report("the quick terminal's hotkey", format!("no key for {chord}"));
            continue;
        };
        let id = EventHotKeyID {
            signature: SIGNATURE,
            id: u32::try_from(index).unwrap_or(0),
        };
        let mut hot_key: EventHotKeyRef = std::ptr::null_mut();
        // SAFETY: The application's event target lives as long as the app; `hot_key` receives the registration.
        let status = unsafe {
            RegisterEventHotKey(
                u32::from(code),
                modifiers(chord),
                id,
                GetApplicationEventTarget(),
                0,
                &raw mut hot_key,
            )
        };
        if status == NO_ERR {
            REGISTERED.with(|registered| registered.borrow_mut().push(hot_key));
        } else {
            term_base::diag::report(
                "the quick terminal's hotkey",
                format!("{chord} is taken (status {status})"),
            );
        }
    }
}

fn install_handler() {
    if INSTALLED.replace(true) {
        return;
    }
    let spec = EventTypeSpec {
        event_class: K_EVENT_CLASS_KEYBOARD,
        event_kind: K_EVENT_HOT_KEY_PRESSED,
    };
    // SAFETY: The handler is a plain function for the life of the app; the spec is read during the call.
    let status = unsafe {
        InstallEventHandler(
            GetApplicationEventTarget(),
            pressed,
            1,
            &raw const spec,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if status != NO_ERR {
        term_base::diag::report("the quick terminal's hotkey", format!("no handler (status {status})"));
    }
}

/// Carbon calls this on the main thread for each press of a registered hotkey.
extern "C" fn pressed(_call: EventHandlerCallRef, _event: EventRef, _data: *mut c_void) -> OSStatus {
    term_base::guarded((), super::toggle);
    NO_ERR
}

fn modifiers(chord: &Chord) -> u32 {
    [
        (chord.cmd, CMD_KEY),
        (chord.shift, SHIFT_KEY),
        (chord.alt, OPTION_KEY),
        (chord.ctrl, CONTROL_KEY),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// The key codes of keys that type no character, the same on every layout.
const NAMED: &[(&str, u16)] = &[
    ("enter", 0x24),
    ("tab", 0x30),
    ("space", 0x31),
    ("backspace", 0x33),
    ("escape", 0x35),
    ("delete", 0x75),
    ("left", 0x7B),
    ("right", 0x7C),
    ("down", 0x7D),
    ("up", 0x7E),
    ("home", 0x73),
    ("end", 0x77),
    ("pageup", 0x74),
    ("pagedown", 0x79),
    ("f1", 0x7A),
    ("f2", 0x78),
    ("f3", 0x63),
    ("f4", 0x76),
    ("f5", 0x60),
    ("f6", 0x61),
    ("f7", 0x62),
    ("f8", 0x64),
    ("f9", 0x65),
    ("f10", 0x6D),
    ("f11", 0x67),
    ("f12", 0x6F),
];

/// The key code of `key`: a named key's, or the key that types the character on the layout in use.
fn key_code(key: &str) -> Option<u16> {
    if let Some((_, code)) = NAMED.iter().find(|(name, _)| *name == key) {
        return Some(*code);
    }
    (0u16..128)
        .filter(|code| !NAMED.iter().any(|(_, named)| named == code))
        .find(|code| typed_by(*code).is_some_and(|typed| typed.to_lowercase() == key))
}

/// What key `code` types without modifiers on the layout in use.
fn typed_by(code: u16) -> Option<String> {
    let event = CGEvent::new_keyboard_event(None, code, true)?;
    let mut units = [0u16; 4];
    let mut length = 0;
    // SAFETY: The buffer holds the four units the call is told it may write; `length` receives how many it wrote.
    unsafe {
        CGEvent::keyboard_get_unicode_string(Some(&event), units.len() as _, &raw mut length, units.as_mut_ptr())
    };
    let length = usize::try_from(length).ok()?.min(units.len());
    let text = String::from_utf16(&units[..length]).ok()?;
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_map_to_carbons_bits() {
        let chord: Chord = "ctrl+alt+cmd+space".parse().unwrap();
        assert_eq!(modifiers(&chord), CONTROL_KEY | OPTION_KEY | CMD_KEY);
        assert_eq!(key_code(&chord.key), Some(0x31));
    }

    #[test]
    fn every_named_key_of_a_chord_has_a_code() {
        for name in ["enter", "tab", "space", "escape", "left", "pageup", "f1", "f12"] {
            assert!(key_code(name).is_some(), "{name}");
        }
    }
}
