//! The AppKit shorthands of `term_ui::kit` and the main-thread hops of `term_base`, with what only the agent tab
//! needs beside them: a thread for one piece of work, one edit of a text storage, and giving memory back.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use objc2_app_kit::NSTextStorage;
use term_base::LockIgnoringPoison;
use term_base::explain::{Explain, Explained};

pub use term_base::main_thread::after_frame;
pub use term_ui::kit::{
    FlagGuard, acknowledge, activate, auto_layout, borderless_button, center_in, center_y, fixed_size, four_colours,
    guarded, hide_from_voiceover, item, label, live, main_after, main_soon, ns, pin_edges, pin_inset, pin_sides,
    separator, set_action, set_label, set_target, set_title_element, submenu, symbol, symbol_at, symbol_button, tagged,
    tinted,
};

/// Runs `f` on a new thread called `name`; the error says, in the user's words, why the thread did not start.
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Result<(), Explained> {
    spawn_with(name, (), move |()| f()).map_err(|((), why)| why)
}

/// `spawn` that hands `payload` to `f`, or back with the reason when the thread did not start, so that a callback in
/// it can still be told.
pub fn spawn_with<T: Send + 'static>(
    name: &str,
    payload: T,
    f: impl FnOnce(T) + Send + 'static,
) -> Result<(), (T, Explained)> {
    let slot = Arc::new(Mutex::new(Some(payload)));
    let theirs = Arc::clone(&slot);
    let started = term_base::thread::detach(name, move || {
        let payload = theirs.lock_ignoring_poison().take();
        if let Some(payload) = payload {
            f(payload);
        }
    });
    started.map_err(|unstarted| {
        drop(unstarted.work);
        let why = Explained(splat_i18n::tf(
            "Konnte keinen Thread starten: {}",
            &[&unstarted.error.explain()],
        ));
        let payload = slot.lock_ignoring_poison().take();
        (payload.expect("a thread that never started took nothing"), why)
    })
}

/// One edit of a text storage, which it processes once the guard drops, also when a panic unwinds past it.
#[must_use = "the edit ends as soon as the guard drops"]
struct EditingGuard<'a>(&'a NSTextStorage);

impl Drop for EditingGuard<'_> {
    fn drop(&mut self) {
        self.0.endEditing();
    }
}

/// Runs `f` as one edit of `storage`.
pub fn with_editing<R>(storage: &NSTextStorage, f: impl FnOnce() -> R) -> R {
    storage.beginEditing();
    let _editing = EditingGuard(storage);
    f()
}

unsafe extern "C" {
    /// libmalloc: hands the free pages of `zone`, or of every zone when null, back to the system, up to `goal` bytes
    /// or all of them when 0.
    fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
}

/// How long after a large release its pages are handed back, so that what went with it later is gone too.
const SETTLE: Duration = Duration::from_secs(2);

/// Hands the pages that no longer hold anything back to macOS shortly, on a thread of its own; for after a large
/// release such as a closed tab, since the allocator otherwise keeps them until the system runs short.
pub fn give_back_memory_soon() {
    let spawned = spawn("give-back", || {
        std::thread::sleep(SETTLE);
        // SAFETY: A null zone stands for every zone; the call returns only pages that no allocation uses.
        unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
    });
    if let Err(why) = spawned {
        term_base::diag::report("give-back", why);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawned_thread_gets_its_payload() {
        let (sent, received) = std::sync::mpsc::channel();
        spawn_with("payload", 7, move |payload| sent.send(payload).unwrap())
            .map_err(|(_, why)| why.0)
            .unwrap();
        assert_eq!(received.recv().unwrap(), 7);
    }
}
