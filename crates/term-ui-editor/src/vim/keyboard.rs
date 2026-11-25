//! Whether the keyboard layout in use types ASCII, which the system says of every input source: only on one that does
//! not, such as Russian, Greek or Hebrew, does Vim read a character as the key at its place on the US layout (E105).

use std::ffi::c_void;

// SAFETY: The declarations match the Text Input Source Services functions and constant of Carbon's HIToolbox.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(source: *const c_void, key: *const c_void) -> *const c_void;
    static kTISPropertyInputSourceIsASCIICapable: *const c_void;
}

// SAFETY: The declarations match CoreFoundation's functions of these names.
unsafe extern "C" {
    fn CFBooleanGetValue(boolean: *const c_void) -> u8;
    fn CFRelease(cf: *const c_void);
}

/// Whether the keyboard input source in use types ASCII, as German or US do; one the system cannot tell counts as one
/// that does, so that a typed character reaches Vim as it is.
pub(super) fn types_ascii() -> bool {
    // SAFETY: Takes nothing and returns an input source the caller owns, or null; called on the main thread, as the
    // Text Input Sources API asks, from the editor's handling of a key.
    let source = unsafe { TISCopyCurrentKeyboardInputSource() };
    if source.is_null() {
        return true;
    }
    // SAFETY: `source` is the live input source from above, and the key is the framework's constant, which lives as
    // long as the process; the value returned belongs to `source`.
    let value = unsafe { TISGetInputSourceProperty(source, kTISPropertyInputSourceIsASCIICapable) };
    // SAFETY: This property's value is a CFBoolean, read while `source` still holds it.
    let ascii = value.is_null() || unsafe { CFBooleanGetValue(value) } != 0;
    // SAFETY: The input source was copied above and is released once.
    unsafe { CFRelease(source) };
    ascii
}
