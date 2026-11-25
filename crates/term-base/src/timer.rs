//! One thread for every deadline of the app's crates, so that waiting for a deadline costs no thread of its own.

use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::sync::{IgnoringPoison, LockIgnoringPoison};

type Fire = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Timers {
    due: BTreeMap<(Instant, u64), Fire>,
    next: u64,
    running: bool,
}

struct Shared {
    timers: Mutex<Timers>,
    changed: Condvar,
}

fn shared() -> &'static Shared {
    static SHARED: OnceLock<Shared> = OnceLock::new();
    SHARED.get_or_init(|| Shared {
        timers: Mutex::default(),
        changed: Condvar::new(),
    })
}

/// Calls `fire` on the timer thread once `delay` has passed. Without a timer thread, as when the system has no
/// thread left, `fire` runs at once, so that whoever waits for the deadline still hears of it.
///
/// ```
/// use std::sync::mpsc::channel;
/// use std::time::{Duration, Instant};
///
/// let (tx, rx) = channel();
/// let start = Instant::now();
/// term_base::timer::after(Duration::from_millis(20), move || tx.send(Instant::now()).unwrap());
/// assert!(rx.recv().unwrap() - start >= Duration::from_millis(20));
/// ```
pub fn after(delay: Duration, fire: impl FnOnce() + Send + 'static) {
    let at = Instant::now().checked_add(delay).unwrap_or_else(far_future);
    let shared = shared();
    let start = {
        let mut timers = shared.timers.lock_ignoring_poison();
        timers.next += 1;
        let key = (at, timers.next);
        timers.due.insert(key, Box::new(fire));
        !std::mem::replace(&mut timers.running, true)
    };
    shared.changed.notify_all();
    if start && let Err(unstarted) = crate::thread::detach("timer", run) {
        crate::diag::report("timer", &unstarted.error);
        let fired: Vec<Fire> = {
            let mut timers = shared.timers.lock_ignoring_poison();
            timers.running = false;
            std::mem::take(&mut timers.due).into_values().collect()
        };
        for fire in fired {
            crate::lane::contained(fire);
        }
    }
}

fn far_future() -> Instant {
    Instant::now() + Duration::from_secs(100 * 365 * 24 * 3600)
}

fn run() {
    let shared = shared();
    let mut timers = shared.timers.lock_ignoring_poison();
    loop {
        let now = Instant::now();
        let due: Vec<(Instant, u64)> = timers.due.range(..=(now, u64::MAX)).map(|(key, _)| *key).collect();
        if !due.is_empty() {
            let fired: Vec<Fire> = due.iter().filter_map(|key| timers.due.remove(key)).collect();
            drop(timers);
            for fire in fired {
                crate::lane::contained(fire);
            }
            timers = shared.timers.lock_ignoring_poison();
            continue;
        }
        timers = match timers.due.keys().next() {
            Some((at, _)) => {
                let wait = at.saturating_duration_since(now);
                shared.changed.wait_timeout(timers, wait).ignoring_poison().0
            }
            None => shared.changed.wait(timers).ignoring_poison(),
        };
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn timers_fire_in_the_order_of_their_deadlines() {
        let (tx, rx) = channel();
        for (delay, name) in [(60, "late"), (10, "early"), (30, "middle")] {
            let tx = tx.clone();
            after(Duration::from_millis(delay), move || tx.send(name).unwrap());
        }
        let order: Vec<&str> = (0..3)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        assert_eq!(order, ["early", "middle", "late"]);
    }

    #[test]
    fn a_panicking_timer_leaves_the_others_running() {
        after(Duration::ZERO, || panic!("a timer that panics"));
        let (tx, rx) = channel();
        after(Duration::from_millis(5), move || tx.send(()).unwrap());
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
