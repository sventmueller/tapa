//! What goes wrong out of the user's sight in a crate without a window — a frame that does not decode, a message of a
//! known type that fails to parse, an error nobody waits for — reported through one hook the app installs. Each kind
//! of report is limited to a few per minute, so a server that sends garbage in a loop cannot flood the system log.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::sync::LockIgnoringPoison;

/// Where reports go: what failed, then the detail.
pub(crate) type Hook = Box<dyn Fn(&str, &str) + Send + Sync>;

/// How many reports of one kind pass per window.
pub(crate) const BURST: u32 = 5;

/// The window over which [`BURST`] is counted.
pub(crate) const WINDOW: Duration = Duration::from_secs(60);

static HOOK: OnceLock<Hook> = OnceLock::new();
static SEEN: OnceLock<Mutex<HashMap<String, Count>>> = OnceLock::new();

struct Count {
    since: Instant,
    passed: u32,
    held_back: u32,
}

/// Sends every later report to `hook`; the first installation wins. Until then reports go to standard error.
pub fn install(hook: Hook) {
    let _ = HOOK.set(hook);
}

/// Reports that `what` failed with `detail`, unless reports of `what` already used up this window.
pub fn report(what: &str, detail: impl Display) {
    let Some(held_back) = admit(what, Instant::now()) else {
        return;
    };
    let detail = if held_back == 0 {
        detail.to_string()
    } else {
        format!("{detail} ({held_back} more like it held back)")
    };
    match HOOK.get() {
        Some(hook) => hook(what, &detail),
        None => eprintln!("{what}: {detail}"),
    }
}

/// `Some(held back since the last report)` when a report of `what` may pass at `now`.
fn admit(what: &str, now: Instant) -> Option<u32> {
    let mut seen = SEEN.get_or_init(Mutex::default).lock_ignoring_poison();
    let count = seen.entry(what.to_owned()).or_insert(Count {
        since: now,
        passed: 0,
        held_back: 0,
    });
    if now.duration_since(count.since) >= WINDOW {
        count.since = now;
        count.passed = 0;
    }
    if count.passed >= BURST {
        count.held_back += 1;
        return None;
    }
    count.passed += 1;
    Some(std::mem::take(&mut count.held_back))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_passes_then_the_rest_waits_for_the_next_window() {
        let start = Instant::now();
        let what = "diag test burst";
        for _ in 0..BURST {
            assert_eq!(admit(what, start), Some(0));
        }
        assert_eq!(admit(what, start), None);
        assert_eq!(admit(what, start + Duration::from_secs(1)), None);
        assert_eq!(admit(what, start + WINDOW), Some(2));
        assert_eq!(admit(what, start + WINDOW), Some(0));
    }

    #[test]
    fn kinds_are_counted_apart() {
        let now = Instant::now();
        for _ in 0..BURST {
            assert!(admit("diag test one", now).is_some());
        }
        assert!(admit("diag test one", now).is_none());
        assert!(admit("diag test two", now).is_some());
    }
}
