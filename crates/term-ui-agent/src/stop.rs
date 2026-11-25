//! Stopping a turn only on purpose: a press of Esc or ⌘. that reaches the tab, the stop button, or the stop in the
//! activity list, never a command that merely arrives as `cancelOperation:` or `complete:`; and a notice at the
//! window's foot that says which one stopped it.

use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSEventType};
use splat_i18n::t;

/// The key code of Esc.
const ESCAPE: u16 = 53;
/// The key code of the full stop, which with ⌘ cancels on macOS.
const PERIOD: u16 = 47;

/// How long after a message went out the send button, which turns into the stop button, ignores a click, so a
/// second click of the one meant to send does not stop the turn it started.
const AFTER_SEND: Duration = Duration::from_millis(1500);

/// What stopped a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Esc or ⌘. in the tab.
    Key,
    /// The stop button beside the composer.
    Button,
    /// The stop of the turn in the activity list.
    Activity,
}

impl Stop {
    /// What the notice at the window's foot says.
    pub(crate) fn notice(self) -> &'static str {
        match self {
            Stop::Key => t("Gestoppt mit Esc"),
            Stop::Button => t("Gestoppt mit dem Stopp-Knopf"),
            Stop::Activity => t("Gestoppt in der Aktivität"),
        }
    }
}

/// Writes down that `by` stopped the turn of the tab `title` in `~/Library/Logs`, as the window's notice fades and the
/// system log keeps the app's lines only a short while; a stop that ends work nobody meant to end can be traced then.
pub(crate) fn record(by: Stop, title: &str) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let line = format!("{seconds} {by:?} {title}\n");
    let queued = term_base::lane::DISK.try_run(move || {
        let dir = term_base::logs_dir();
        let written = std::fs::create_dir_all(&dir).and_then(|()| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("stops.log"))?
                .write_all(line.as_bytes())
        });
        if let Err(error) = written {
            term_base::diag::report("writing down a stop", error);
        }
    });
    if queued.is_err() {
        term_base::diag::report("writing down a stop", "the disk lane is full");
    }
}

/// Whether a key with `code` pressed with `flags` means stop: Esc on its own, or ⌘.
pub(crate) fn is_stop_key(code: u16, flags: NSEventModifierFlags) -> bool {
    let held = flags
        & (NSEventModifierFlags::Command
            | NSEventModifierFlags::Option
            | NSEventModifierFlags::Control
            | NSEventModifierFlags::Shift);
    (code == ESCAPE && held.is_empty()) || (code == PERIOD && held == NSEventModifierFlags::Command)
}

/// Whether the event the app handles now is a press of a key that means stop.
pub(crate) fn stop_key_pressed(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|event| {
            event.r#type() == NSEventType::KeyDown && is_stop_key(event.keyCode(), event.modifierFlags())
        })
}

/// Whether a click of the stop button at `now` comes too soon after the message sent at `sent` to mean stop.
pub(crate) fn too_soon_after_send(sent: Option<Instant>, now: Instant) -> bool {
    sent.is_some_and(|sent| now.saturating_duration_since(sent) < AFTER_SEND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_esc_alone_and_command_period_mean_stop() {
        assert!(is_stop_key(ESCAPE, NSEventModifierFlags::empty()));
        assert!(
            is_stop_key(ESCAPE, NSEventModifierFlags::CapsLock),
            "caps lock is no modifier one holds"
        );
        assert!(is_stop_key(PERIOD, NSEventModifierFlags::Command));
        assert!(
            !is_stop_key(ESCAPE, NSEventModifierFlags::Option),
            "⌥Esc asks for completions"
        );
        assert!(!is_stop_key(ESCAPE, NSEventModifierFlags::Command));
        assert!(!is_stop_key(PERIOD, NSEventModifierFlags::empty()), "a typed full stop");
        assert!(
            !is_stop_key(96, NSEventModifierFlags::empty()),
            "F5 asks for completions too"
        );
    }

    #[test]
    fn a_second_click_right_after_sending_does_not_stop() {
        let now = Instant::now();
        assert!(too_soon_after_send(Some(now), now + Duration::from_millis(300)));
        assert!(!too_soon_after_send(Some(now), now + AFTER_SEND));
        assert!(!too_soon_after_send(None, now));
    }
}
