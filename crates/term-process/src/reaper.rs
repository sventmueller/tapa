//! One thread for the whole app that hears through kqueue when processes end: it reaps the children the supervisor
//! started and wakes whoever waits for a process or a whole group to be gone.

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ExitStatus};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use term_base::{IgnoringPoison, LockIgnoringPoison};

use crate::Ending;

type OnEnd = Box<dyn FnOnce(Ending) + Send>;

/// How often a wait looks again at processes it cannot watch, such as one that is about to be reaped.
const POLL: Duration = Duration::from_millis(20);

/// How often the reaper also asks its children directly, in case the system never reported one's exit.
const SWEEP: Duration = Duration::from_secs(10);

/// How many endings one call to kevent hands over at most.
const BATCH: usize = 16;

struct Reaper {
    queue: libc::c_int,
    waiting: Mutex<HashMap<libc::pid_t, Waiting>>,
}

/// Who waits for one process number: the supervisor, which reaps it, and any number of bells.
#[derive(Default)]
struct Waiting {
    reap: Option<OnEnd>,
    bells: Vec<Arc<Bell>>,
}

impl Waiting {
    fn is_empty(&self) -> bool {
        self.reap.is_none() && self.bells.is_empty()
    }
}

/// The app's reaper; `None` when the system gave it no kqueue or no thread, and waiting falls back to a thread per
/// child and to looking again now and then.
fn reaper() -> Option<&'static Reaper> {
    static REAPER: OnceLock<Option<Reaper>> = OnceLock::new();
    REAPER.get_or_init(Reaper::start).as_ref()
}

impl Reaper {
    fn start() -> Option<Reaper> {
        // SAFETY: kqueue takes no arguments and returns a new descriptor or -1.
        let queue = unsafe { libc::kqueue() };
        if queue < 0 {
            term_base::diag::report("starting the reaper", io::Error::last_os_error());
            return None;
        }
        let started = std::thread::Builder::new()
            .name("reaper".into())
            .spawn(move || serve(queue));
        match started {
            Ok(_) => Some(Reaper {
                queue,
                waiting: Mutex::new(HashMap::new()),
            }),
            Err(error) => {
                term_base::diag::report("starting the reaper", error);
                // SAFETY: The descriptor was just opened here and nothing else knows it.
                unsafe { libc::close(queue) };
                None
            }
        }
    }

    /// Asks the queue to report the end of `pid` once.
    fn arm(&self, pid: libc::pid_t) -> io::Result<()> {
        let change = change(pid, libc::EV_ADD | libc::EV_ONESHOT);
        // SAFETY: The change list is one live kevent and the event list is empty, so kevent reads one and writes none;
        // an error comes back through errno.
        let status = unsafe {
            libc::kevent(
                self.queue,
                &raw const change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if status < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn disarm(&self, pid: libc::pid_t) {
        let change = change(pid, libc::EV_DELETE);
        // SAFETY: As in `arm`; a filter that already fired and is gone only makes the call fail.
        unsafe {
            libc::kevent(
                self.queue,
                &raw const change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
    }

    /// Reaps `child` once it ends; `Err` hands both back when the queue refuses it.
    fn reap(&self, child: Child, on_end: OnEnd) -> Result<Option<Ending>, (Child, OnEnd)> {
        let Ok(pid) = libc::pid_t::try_from(child.id()) else {
            return Err((child, on_end));
        };
        let mut waiting = self.waiting.lock_ignoring_poison();
        let entry = waiting.entry(pid).or_default();
        let fresh = entry.is_empty();
        entry.reap = Some(on_end);
        let armed = if fresh { self.arm(pid) } else { Ok(()) };
        match armed {
            Ok(()) => {
                // A child that ended before the queue knew it may never be reported, so it is asked once here.
                match reaped(pid, libc::WNOHANG) {
                    Some(ending) => {
                        let entry = waiting.remove(&pid).unwrap_or_default();
                        self.disarm(pid);
                        drop(waiting);
                        ring(entry.bells);
                        Ok(Some(ending))
                    }
                    None => Ok(None),
                }
            }
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                let entry = waiting.remove(&pid).unwrap_or_default();
                drop(waiting);
                ring(entry.bells);
                Ok(Some(reaped(pid, 0).unwrap_or(Ending::Unknown)))
            }
            Err(_) => {
                let on_end = waiting.remove(&pid).and_then(|entry| entry.reap);
                drop(waiting);
                Err((child, on_end.unwrap_or_else(|| Box::new(drop))))
            }
        }
    }

    /// Lets `bell` ring when `pid` ends.
    fn watch(&self, pid: libc::pid_t, bell: &Arc<Bell>) -> Watched {
        let mut waiting = self.waiting.lock_ignoring_poison();
        let entry = waiting.entry(pid).or_default();
        if entry.bells.iter().any(|b| Arc::ptr_eq(b, bell)) {
            return Watched::Armed;
        }
        let fresh = entry.is_empty();
        entry.bells.push(bell.clone());
        if !fresh {
            return Watched::Armed;
        }
        match self.arm(pid) {
            Ok(()) => Watched::Armed,
            Err(error) => {
                waiting.remove(&pid);
                if error.raw_os_error() == Some(libc::ESRCH) {
                    Watched::Gone
                } else {
                    Watched::Unwatchable
                }
            }
        }
    }

    fn unwatch(&self, pid: libc::pid_t, bell: &Arc<Bell>) {
        let mut waiting = self.waiting.lock_ignoring_poison();
        let Some(entry) = waiting.get_mut(&pid) else {
            return;
        };
        entry.bells.retain(|b| !Arc::ptr_eq(b, bell));
        if entry.is_empty() {
            waiting.remove(&pid);
            self.disarm(pid);
        }
    }

    /// Hands the end of `pid` to whoever waits for it.
    fn ended(&self, pid: libc::pid_t) {
        let Some(entry) = self.waiting.lock_ignoring_poison().remove(&pid) else {
            return;
        };
        if let Some(on_end) = entry.reap {
            let ending = reaped(pid, 0).unwrap_or(Ending::Unknown);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_end(ending)));
        }
        ring(entry.bells);
    }

    /// Reaps the children whose end was not reported.
    fn sweep(&self) {
        let mut found = Vec::new();
        {
            let mut waiting = self.waiting.lock_ignoring_poison();
            let children: Vec<libc::pid_t> = waiting
                .iter()
                .filter(|(_, entry)| entry.reap.is_some())
                .map(|(&pid, _)| pid)
                .collect();
            for pid in children {
                if let Some(ending) = reaped(pid, libc::WNOHANG)
                    && let Some(entry) = waiting.remove(&pid)
                {
                    self.disarm(pid);
                    found.push((ending, entry));
                }
            }
        }
        for (ending, entry) in found {
            if let Some(on_end) = entry.reap {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_end(ending)));
            }
            ring(entry.bells);
        }
    }
}

fn change(pid: libc::pid_t, flags: u16) -> libc::kevent {
    libc::kevent {
        ident: usize::try_from(pid).unwrap_or(0),
        filter: libc::EVFILT_PROC,
        flags,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}

fn ring(bells: Vec<Arc<Bell>>) {
    for bell in bells {
        bell.ring();
    }
}

/// The reaper's thread: waits on the queue for as long as the app runs.
fn serve(queue: libc::c_int) {
    term_base::priority::set_qos(term_base::priority::Qos::Utility);
    let mut events = [change(0, 0); BATCH];
    let sweep = libc::timespec {
        tv_sec: libc::time_t::try_from(SWEEP.as_secs()).unwrap_or(10),
        tv_nsec: 0,
    };
    let capacity = libc::c_int::try_from(BATCH).unwrap_or(1);
    loop {
        // SAFETY: The event list holds `capacity` kevents and outlives the call, which writes at most that many; the
        // change list is empty and the timeout a live value.
        let count = unsafe {
            libc::kevent(
                queue,
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                capacity,
                &raw const sweep,
            )
        };
        let Some(reaper) = reaper() else {
            return;
        };
        match usize::try_from(count) {
            Ok(0) => reaper.sweep(),
            Ok(count) => {
                for event in &events[..count.min(BATCH)] {
                    if event.filter == libc::EVFILT_PROC
                        && event.fflags & libc::NOTE_EXIT != 0
                        && let Ok(pid) = libc::pid_t::try_from(event.ident)
                    {
                        reaper.ended(pid);
                    }
                }
            }
            Err(_) => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    term_base::diag::report("waiting for processes to end", error);
                    std::thread::sleep(POLL);
                }
            }
        }
    }
}

/// How the child `pid` ended, reaping it; `None` while it runs, with `WNOHANG` in `options`.
fn reaped(pid: libc::pid_t, options: libc::c_int) -> Option<Ending> {
    let mut status: libc::c_int = 0;
    loop {
        // SAFETY: waitpid writes one c_int to the live local; `pid` is a child of this process not yet reaped, since
        // only the reaper reaps the children it was handed.
        let result = unsafe { libc::waitpid(pid, &raw mut status, options) };
        if result == pid {
            return Some(ending(ExitStatus::from_raw(status)));
        }
        if result == 0 {
            return None;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Some(Ending::Unknown);
        }
    }
}

/// How a process with `status` ended.
pub(crate) fn ending(status: ExitStatus) -> Ending {
    match (status.code(), status.signal()) {
        (Some(code), _) => Ending::Code(code),
        (None, Some(signal)) => Ending::Signal(signal),
        (None, None) => Ending::Unknown,
    }
}

/// Reaps `child` when it ends and hands how it ended to `on_end`, on the reaper's thread; `Some` with the ending when
/// it had ended already, and then `on_end` is not called.
pub(crate) fn reap(child: Child, on_end: impl FnOnce(Ending) + Send + 'static) -> io::Result<Option<Ending>> {
    let on_end: OnEnd = Box::new(on_end);
    let (child, on_end) = match reaper() {
        Some(reaper) => match reaper.reap(child, on_end) {
            Ok(ending) => return Ok(ending),
            Err(refused) => refused,
        },
        None => (child, on_end),
    };
    wait_on_thread(child, on_end).map(|()| None)
}

fn wait_on_thread(mut child: Child, on_end: OnEnd) -> io::Result<()> {
    let name = format!("wait-{}", child.id());
    std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            term_base::priority::set_qos(term_base::priority::Qos::Utility);
            let ending = child.wait().map_or(Ending::Unknown, ending);
            on_end(ending);
        })
        .map(drop)
}

/// Rings each time a process it watches ends.
#[derive(Default)]
struct Bell {
    rung: Mutex<u64>,
    changed: Condvar,
}

impl Bell {
    fn ring(&self) {
        *self.rung.lock_ignoring_poison() += 1;
        self.changed.notify_all();
    }

    fn count(&self) -> u64 {
        *self.rung.lock_ignoring_poison()
    }

    /// Waits until it rang more than `seen` times in all, or until `until`.
    fn wait_past(&self, seen: u64, until: Instant) {
        let rung = self.rung.lock_ignoring_poison();
        let timeout = until.saturating_duration_since(Instant::now());
        drop(
            self.changed
                .wait_timeout_while(rung, timeout, |rung| *rung == seen)
                .ignoring_poison(),
        );
    }
}

enum Watched {
    Armed,
    /// It had ended already, or ends but is not reaped yet.
    Gone,
    /// The system does not report its end to the app.
    Unwatchable,
}

/// Waits until `done` holds or `timeout` passes, and looks again each time one of the processes `watched` lists ends;
/// whether `done` held.
pub(crate) fn wait_until(
    timeout: Duration,
    mut watched: impl FnMut() -> Vec<i32>,
    mut done: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    let bell = Arc::new(Bell::default());
    let mut touched = HashSet::new();
    let held = loop {
        let seen = bell.count();
        let pids = watched();
        let mut blind = pids.is_empty();
        for &pid in &pids {
            let watch = match reaper() {
                Some(reaper) if pid > 0 => reaper.watch(pid, &bell),
                _ => Watched::Unwatchable,
            };
            match watch {
                Watched::Armed => {
                    touched.insert(pid);
                }
                Watched::Gone | Watched::Unwatchable => blind = true,
            }
        }
        if done() {
            break true;
        }
        let now = Instant::now();
        if now >= deadline {
            break false;
        }
        let until = if blind { deadline.min(now + POLL) } else { deadline };
        bell.wait_past(seen, until);
    };
    if let Some(reaper) = reaper() {
        for pid in touched {
            reaper.unwatch(pid, &bell);
        }
    }
    held
}

/// Whether nothing is left in process group `group` within `timeout`.
pub fn group_ends_within(group: i32, timeout: Duration) -> bool {
    wait_until(
        timeout,
        || term_base::system::group_members(group),
        || !term_base::system::group_exists(group),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::channel;

    use super::*;

    const LIMIT: Duration = Duration::from_secs(5);

    #[test]
    fn one_thread_reaps_many_children_and_reports_each_end() {
        let (tx, rx) = channel();
        for code in 0..20 {
            let child = Command::new("/bin/sh")
                .args(["-c", &format!("sleep 0.1; exit {code}")])
                .spawn()
                .unwrap();
            let pid = child.id();
            let tx = tx.clone();
            let ended = reap(child, move |ending| tx.send((pid, ending)).unwrap()).unwrap();
            assert_eq!(ended, None, "the child still sleeps");
        }
        let mut codes: Vec<Ending> = (0..20).map(|_| rx.recv_timeout(LIMIT).unwrap().1).collect();
        codes.sort_by_key(|e| match e {
            Ending::Code(c) => *c,
            _ => -1,
        });
        assert_eq!(codes, (0..20).map(Ending::Code).collect::<Vec<_>>());
    }

    #[test]
    fn a_child_that_ended_before_it_was_handed_over_is_reported_at_once() {
        let child = Command::new("/bin/sh").args(["-c", "exit 7"]).spawn().unwrap();
        let pid = libc::id_t::from(child.id());
        // SAFETY: siginfo_t is plain data for which all zero bytes are a valid value.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waitid writes one siginfo_t to the live local; WNOWAIT leaves the ended child a zombie, unreaped.
        let status = unsafe { libc::waitid(libc::P_PID, pid, &raw mut info, libc::WEXITED | libc::WNOWAIT) };
        assert_eq!(status, 0);
        let (tx, rx) = channel::<Ending>();
        let ended = reap(child, move |e| tx.send(e).unwrap()).unwrap();
        assert_eq!(ended, Some(Ending::Code(7)));
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "on_end is not called as well"
        );
    }

    #[test]
    fn waiting_for_a_group_ends_as_soon_as_its_last_member_does() {
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "sleep 0.3 & echo $!; wait"])
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(leader.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let group = i32::try_from(leader.id()).unwrap();
        let waiter = std::thread::spawn(move || leader.wait());
        let started = Instant::now();
        assert!(group_ends_within(group, LIMIT));
        assert!(started.elapsed() < Duration::from_secs(2));
        waiter.join().unwrap().unwrap();
    }

    #[test]
    fn waiting_for_a_group_that_stays_gives_up_at_the_limit() {
        let mut leader = Command::new("/bin/sleep").arg("30").process_group(0).spawn().unwrap();
        let group = i32::try_from(leader.id()).unwrap();
        let started = Instant::now();
        assert!(!group_ends_within(group, Duration::from_millis(200)));
        assert!(started.elapsed() >= Duration::from_millis(200));
        leader.kill().unwrap();
        leader.wait().unwrap();
    }

    #[test]
    fn a_wait_that_holds_from_the_start_returns_at_once() {
        let started = Instant::now();
        assert!(wait_until(LIMIT, Vec::new, || true));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(group_ends_within(1, LIMIT), "group 1 is never the app's");
    }
}
