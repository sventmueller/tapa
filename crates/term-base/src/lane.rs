//! Worker threads with a limit on waiting jobs, and code run so that a panic costs one result and not the app.

use std::io;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::sync::{IgnoringPoison, LockIgnoringPoison};

type Job = Box<dyn FnOnce() + Send>;

/// Threads that run jobs one after the other, started by the first job. A job that panics ends only itself.
pub struct Lane {
    name: &'static str,
    workers: usize,
    capacity: usize,
    start: fn(),
    jobs: Mutex<Option<SyncSender<Job>>>,
}

/// Why a lane took no job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The lane already holds as many waiting jobs as it takes.
    Full,
    /// No thread runs the lane's jobs, because the system had none to give; a later job tries again.
    Stopped,
}

impl Lane {
    /// A lane of `workers` threads called `name` that holds up to `capacity` waiting jobs.
    pub const fn new(name: &'static str, workers: usize, capacity: usize) -> Lane {
        Lane::with_start(name, workers, capacity, nothing)
    }

    /// A lane as `new` whose threads first call `start`, such as to set how urgent their work is.
    pub const fn with_start(name: &'static str, workers: usize, capacity: usize, start: fn()) -> Lane {
        Lane {
            name,
            workers,
            capacity,
            start,
            jobs: Mutex::new(None),
        }
    }

    /// Queues `job`; `Full` when `capacity` jobs are already waiting, so the caller can drop or merge work instead
    /// of piling it up, and `Stopped` when no thread could be started for it.
    pub fn try_run(&self, job: impl FnOnce() + Send + 'static) -> Result<(), Refused> {
        self.try_run_with(Box::new(job), spawn)
    }

    /// Runs `work` on the lane and hands its result, or `None` after a panic, to `answer` there; refused as `try_run`.
    ///
    /// ```
    /// use std::sync::mpsc::channel;
    ///
    /// use term_base::lane::Lane;
    ///
    /// static LANE: Lane = Lane::new("example", 1, 8);
    /// let (tx, rx) = channel();
    /// let failed = tx.clone();
    /// LANE.ask(|| 6 * 7, move |answer| tx.send(answer).unwrap()).unwrap();
    /// LANE.ask(|| -> i32 { panic!("lost") }, move |answer| failed.send(answer).unwrap()).unwrap();
    /// assert_eq!(rx.recv().unwrap(), Some(42));
    /// assert_eq!(rx.recv().unwrap(), None);
    /// ```
    pub fn ask<T>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
        answer: impl FnOnce(Option<T>) + Send + 'static,
    ) -> Result<(), Refused> {
        self.try_run(move || answer(contained(work)))
    }

    /// Runs `work` on the lane and waits for its result, or `None` after a panic, at most `budget`; past that the
    /// result goes to whatever [`Pending::then`] is given. Refused as `try_run`.
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use term_base::lane::{Answer, Lane};
    ///
    /// static LANE: Lane = Lane::new("example-within", 1, 8);
    /// let Ok(Answer::Now(answer)) = LANE.ask_within(Duration::from_secs(5), || 6 * 7) else { panic!() };
    /// assert_eq!(answer, Some(42));
    /// ```
    pub fn ask_within<T: Send + 'static>(
        &self,
        budget: Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<Answer<T>, Refused> {
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot::Waiting),
            ready: Condvar::new(),
        });
        let theirs = Arc::clone(&shared);
        self.try_run(move || theirs.deliver(contained(work)))?;
        let deadline = Instant::now() + budget;
        let mut slot = shared.slot.lock_ignoring_poison();
        loop {
            if matches!(*slot, Slot::Done(_)) {
                let Slot::Done(answer) = std::mem::replace(&mut *slot, Slot::Taken) else {
                    unreachable!("the slot was just seen done");
                };
                return Ok(Answer::Now(answer));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            slot = shared.ready.wait_timeout(slot, left).ignoring_poison().0;
        }
        drop(slot);
        Ok(Answer::Later(Pending(shared)))
    }

    fn try_run_with(&self, job: Job, spawn: impl Fn(String, Job) -> io::Result<()>) -> Result<(), Refused> {
        let mut jobs = self.jobs.lock_ignoring_poison();
        if jobs.is_none() {
            *jobs = self.start_workers(spawn);
        }
        let Some(sender) = jobs.as_ref() else {
            return Err(Refused::Stopped);
        };
        match sender.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(Refused::Full),
            Err(TrySendError::Disconnected(_)) => {
                *jobs = None;
                Err(Refused::Stopped)
            }
        }
    }

    /// The lane's queue with its threads running, or `None` when not one of them started.
    fn start_workers(&self, spawn: impl Fn(String, Job) -> io::Result<()>) -> Option<SyncSender<Job>> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(self.capacity);
        let rx = Arc::new(Mutex::new(rx));
        let mut started = 0;
        for n in 0..self.workers.max(1) {
            let rx = rx.clone();
            let start = self.start;
            let name = if self.workers <= 1 {
                self.name.to_string()
            } else {
                format!("{}-{n}", self.name)
            };
            let worker: Job = Box::new(move || {
                start();
                while let Ok(Ok(job)) = rx.lock().map(|rx| {
                    crate::main_guard::blocking("a lane's next job");
                    rx.recv()
                }) {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                }
            });
            match spawn(name, worker) {
                Ok(()) => started += 1,
                Err(error) => crate::diag::report(&format!("lane {}", self.name), &error),
            }
        }
        (started > 0).then_some(tx)
    }
}

/// What `Lane::ask_within` got within its budget.
pub enum Answer<T> {
    /// The result came in time; `None` if the work panicked.
    Now(Option<T>),
    /// The work still runs.
    Later(Pending<T>),
}

/// The result of work that outlasted the wait for it.
pub struct Pending<T>(Arc<Shared<T>>);

impl<T> Pending<T> {
    /// Hands the result, or `None` after a panic, to `late`: on the lane once the work is done, or right here if it
    /// already is.
    pub fn then(self, late: impl FnOnce(Option<T>) + Send + 'static) {
        let mut slot = self.0.slot.lock_ignoring_poison();
        if let Slot::Done(_) = *slot {
            let Slot::Done(answer) = std::mem::replace(&mut *slot, Slot::Taken) else {
                unreachable!("the slot was just seen done");
            };
            drop(slot);
            return late(answer);
        }
        *slot = Slot::Late(Box::new(late));
    }
}

type Late<T> = Box<dyn FnOnce(Option<T>) + Send>;

struct Shared<T> {
    slot: Mutex<Slot<T>>,
    ready: Condvar,
}

enum Slot<T> {
    Waiting,
    Done(Option<T>),
    Late(Late<T>),
    Taken,
}

impl<T> Shared<T> {
    fn deliver(&self, answer: Option<T>) {
        let mut slot = self.slot.lock_ignoring_poison();
        if let Slot::Late(_) = *slot {
            let Slot::Late(late) = std::mem::replace(&mut *slot, Slot::Taken) else {
                unreachable!("the slot was just seen waiting late");
            };
            drop(slot);
            return late(answer);
        }
        *slot = Slot::Done(answer);
        self.ready.notify_all();
    }
}

fn spawn(name: String, worker: Job) -> io::Result<()> {
    std::thread::Builder::new().name(name).spawn(worker).map(drop)
}

fn nothing() {}

/// Runs `f`; `None` if it panicked.
pub fn contained<T>(f: impl FnOnce() -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).ok()
}

/// Reading and writing the user's files: a network volume that stops answering may hold it for minutes.
pub static DISK: Lane = Lane::new("term-disk", 1, 256);

/// Computation off the main thread.
pub static WORK: Lane = Lane::new("term-work", 3, 256);

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_panicking_job_ends_only_itself() {
        static LANE: Lane = Lane::new("test-panic", 1, 4);
        let (tx, rx) = channel();
        LANE.ask(|| panic!("in the job"), {
            let tx = tx.clone();
            move |r: Option<()>| tx.send(r.is_none()).unwrap()
        })
        .unwrap();
        LANE.ask(|| 7, move |r| tx.send(r == Some(7)).unwrap()).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    fn refuse(_: String, _: Job) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::OutOfMemory))
    }

    #[test]
    fn a_lane_without_threads_refuses_and_starts_them_with_a_later_job() {
        static LANE: Lane = Lane::new("test-unstarted", 2, 4);
        assert_eq!(LANE.try_run_with(Box::new(|| ()), refuse), Err(Refused::Stopped));
        assert!(
            LANE.jobs.lock_ignoring_poison().is_none(),
            "nothing is kept of a start that failed"
        );
        let (tx, rx) = channel();
        LANE.ask(|| 7, move |r| tx.send(r).unwrap()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(7));
    }

    #[test]
    fn a_lane_whose_threads_are_gone_says_so_instead_of_full() {
        static LANE: Lane = Lane::new("test-gone", 1, 4);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Job>(4);
        drop(rx);
        *LANE.jobs.lock_ignoring_poison() = Some(tx);
        assert_eq!(LANE.try_run(|| ()), Err(Refused::Stopped));
        LANE.try_run(|| ()).unwrap();
    }

    #[test]
    fn work_that_outlasts_the_wait_answers_later_and_a_panic_answers_none() {
        static LANE: Lane = Lane::new("test-within", 1, 4);
        let gate = Arc::new(Barrier::new(2));
        let held = gate.clone();
        let Ok(Answer::Later(pending)) = LANE.ask_within(Duration::from_millis(5), move || {
            held.wait();
            7
        }) else {
            panic!("the work waits for the gate");
        };
        let (tx, rx) = channel();
        pending.then(move |answer| tx.send(answer).unwrap());
        gate.wait();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Some(7));
        let Ok(Answer::Now(answer)) = LANE.ask_within(Duration::from_secs(5), || -> i32 { panic!("lost") }) else {
            panic!("a panic answers in time");
        };
        assert_eq!(answer, None);
    }

    #[test]
    fn a_result_that_comes_before_then_goes_to_it_right_away() {
        static LANE: Lane = Lane::new("test-within-done", 1, 4);
        let gate = Arc::new(Barrier::new(2));
        let held = gate.clone();
        let (done, finished) = channel();
        let Ok(Answer::Later(pending)) = LANE.ask_within(Duration::ZERO, move || {
            held.wait();
            8
        }) else {
            panic!("no budget waits for nothing");
        };
        gate.wait();
        LANE.try_run(move || done.send(()).unwrap()).unwrap();
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        let (tx, rx) = channel();
        pending.then(move |answer| tx.send(answer).unwrap());
        assert_eq!(rx.try_recv().unwrap(), Some(8), "then ran on the calling thread");
    }

    #[test]
    fn a_full_lane_refuses_instead_of_growing() {
        static LANE: Lane = Lane::new("test-full", 1, 2);
        let gate = Arc::new(Barrier::new(2));
        let held = gate.clone();
        let (started, running) = channel();
        LANE.try_run(move || {
            started.send(()).unwrap();
            held.wait();
        })
        .unwrap();
        running.recv_timeout(Duration::from_secs(5)).unwrap();
        LANE.try_run(|| ()).unwrap();
        LANE.try_run(|| ()).unwrap();
        assert_eq!(LANE.try_run(|| ()), Err(Refused::Full));
        gate.wait();
    }
}
