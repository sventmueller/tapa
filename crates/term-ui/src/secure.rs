//! Secure keyboard entry (F60): while it is on, no other app can read the keys typed into the terminal. It is on while
//! a terminal has the focus and either the user turned it on or the program in front reads a password.

use std::cell::Cell;

use objc2::DefinedClass;
use objc2_foundation::NSUserDefaults;

use crate::kit::ns;

/// The defaults key that keeps the user's choice across launches.
const DEFAULTS_KEY: &str = "SecureKeyboardEntry";

thread_local! {
    static MANUAL: Cell<Option<bool>> = const { Cell::new(None) };
    static ON: Cell<bool> = const { Cell::new(false) };
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
    fn IsSecureEventInputEnabled() -> u8;
}

/// Whether the user turned secure keyboard entry on.
pub fn manual() -> bool {
    MANUAL.with(|manual| {
        manual.get().unwrap_or_else(|| {
            let stored = NSUserDefaults::standardUserDefaults().boolForKey(&ns(DEFAULTS_KEY));
            manual.set(Some(stored));
            stored
        })
    })
}

/// Turns the user's secure keyboard entry on or off and keeps the choice.
pub fn set_manual(on: bool) {
    MANUAL.with(|manual| manual.set(Some(on)));
    NSUserDefaults::standardUserDefaults().setBool_forKey(on, &ns(DEFAULTS_KEY));
    refresh();
}

/// Whether the terminal holds secure keyboard entry on now.
pub fn active() -> bool {
    ON.with(Cell::get)
}

/// Whether the system has secure keyboard entry on, from this app or another.
pub fn system_active() -> bool {
    // SAFETY: A query without arguments.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// Brings secure keyboard entry in line with the focus, the user's choice and a password prompt in front.
pub(crate) fn refresh() {
    let focused = super::all_views()
        .into_iter()
        .find(|view| view.ivars().state.focused.get());
    let wanted = focused.is_some_and(|view| {
        manual()
            || view
                .with_session(term_session::Session::reads_password)
                .unwrap_or(false)
    });
    apply(wanted);
}

/// The system counts every enable, so each one is matched by exactly one disable.
fn apply(wanted: bool) {
    ON.with(|on| {
        if on.get() == wanted {
            return;
        }
        // SAFETY: Calls without arguments; `ON` pairs each enable with one disable.
        unsafe {
            if wanted {
                EnableSecureEventInput();
            } else {
                DisableSecureEventInput();
            }
        }
        on.set(wanted);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_it_on_twice_enables_it_once_and_off_ends_it() {
        apply(true);
        apply(true);
        assert!(active() && system_active());
        apply(false);
        assert!(!active());
        apply(false);
        assert!(!active());
    }
}
