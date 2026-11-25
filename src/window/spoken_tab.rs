//! The title VoiceOver reads for a tab of the native tab bar. The button reads the attributed title it shows, whose
//! icon is an attachment and so the object replacement character U+FFFC, which VoiceOver speaks; AppKit offers no
//! icon before a tab's title other than the attachment, so the button's accessibility title leaves the character out.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;
use term_ui::kit::{guarded, ns};

/// AppKit's private class of a tab's button in the tab bar.
const BUTTON: &std::ffi::CStr = c"NSTabButton";

/// Gives the tab bar's buttons the title without the icon's character; where macOS has no such class, or the class
/// a title of its own, VoiceOver keeps reading the character.
pub(crate) fn install() {
    let Some(class) = AnyClass::get(BUTTON) else { return };
    let Some(inherited) = class.instance_method(sel!(accessibilityTitle)) else {
        return;
    };
    let spoken: extern "C-unwind" fn(&AnyObject, Sel) -> *mut NSString = title;
    // SAFETY: `title` takes the receiver and the selector and returns an object, as `-accessibilityTitle` does; an
    // `Imp` is only a function pointer of unspecified signature.
    let imp: Imp =
        unsafe { std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel) -> *mut NSString, Imp>(spoken) };
    // SAFETY: The class is AppKit's and lives for the process; the type encoding is the inherited method's, which
    // `title` matches. `class_addMethod` adds nothing to a class that has the method itself.
    unsafe {
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            sel!(accessibilityTitle),
            imp,
            objc2::ffi::method_getTypeEncoding(inherited),
        );
    }
}

extern "C-unwind" fn title(button: &AnyObject, _cmd: Sel) -> *mut NSString {
    guarded(std::ptr::null_mut(), || {
        let Some(superclass) = AnyClass::get(BUTTON).and_then(AnyClass::superclass) else {
            return std::ptr::null_mut();
        };
        // SAFETY: The button is an `NSTabButton` or a subclass of it, so its superclass's `-accessibilityTitle`
        // applies to it and returns a string or nil.
        let shown: Option<Retained<NSString>> = unsafe { msg_send![super(button, superclass), accessibilityTitle] };
        match shown {
            Some(shown) => Retained::autorelease_return(ns(spoken(&shown.to_string()))),
            None => std::ptr::null_mut(),
        }
    })
}

/// `title` without the icon's character and the room after it.
fn spoken(title: &str) -> &str {
    title.strip_prefix('\u{FFFC}').map_or(title, str::trim_start)
}

#[cfg(test)]
mod tests {
    use super::spoken;

    #[test]
    fn the_icon_and_the_room_after_it_are_left_out() {
        assert_eq!(spoken("\u{FFFC}\u{2002}~/src"), "~/src");
        assert_eq!(spoken("a.rs"), "a.rs");
        assert_eq!(spoken("a \u{FFFC} b"), "a \u{FFFC} b");
    }
}
