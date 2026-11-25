//! Threads started for one piece of work, whose start can fail when the system has none left; the work then comes
//! back to the caller, who still owes whoever waits on it an answer.

use std::io;
use std::sync::{Arc, Mutex};

use crate::sync::LockIgnoringPoison;

/// Work that no thread could be started for, with the reason.
pub struct Unstarted<W> {
    /// The work, not run.
    pub work: W,
    /// Why no thread started.
    pub error: io::Error,
}

impl<W> std::fmt::Debug for Unstarted<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Unstarted")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Runs `work` on a new thread named `name`, or hands it back when no thread starts.
///
/// ```
/// let (tx, rx) = std::sync::mpsc::channel();
/// let sent = tx.clone();
/// if let Err(unstarted) = term_base::thread::detach("example", move || sent.send(Ok(1)).unwrap()) {
///     drop(unstarted.work);
///     tx.send(Err(unstarted.error.to_string())).unwrap();
/// }
/// assert_eq!(rx.recv().unwrap(), Ok(1));
/// ```
pub fn detach<W>(name: &str, work: W) -> Result<(), Unstarted<W>>
where
    W: FnOnce() + Send + 'static,
{
    detach_with(work, |run| {
        std::thread::Builder::new().name(name.to_owned()).spawn(run).map(drop)
    })
}

fn detach_with<W, S>(work: W, start: S) -> Result<(), Unstarted<W>>
where
    W: FnOnce() + Send + 'static,
    S: FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
{
    let slot = Arc::new(Mutex::new(Some(work)));
    let theirs = Arc::clone(&slot);
    let started = start(Box::new(move || {
        let work = theirs.lock_ignoring_poison().take();
        if let Some(work) = work {
            work();
        }
    }));
    match started {
        Ok(()) => Ok(()),
        Err(error) => match slot.lock_ignoring_poison().take() {
            Some(work) => Err(Unstarted { work, error }),
            None => Ok(()),
        },
    }
}

/// Runs `work` on a new thread named `name`, or right here after reporting it when no thread starts; for work that
/// must happen, such as ending a process or answering a caller who waits.
pub fn detach_or_run<W>(name: &str, work: W)
where
    W: FnOnce() + Send + 'static,
{
    if let Err(unstarted) = detach(name, work) {
        run_unstarted(name, unstarted);
    }
}

/// Runs `work` with `answer` on a new thread named `name`; when no thread starts, `refuse` gets `answer` and the
/// reason right here, so whoever waits for the answer still hears.
pub fn detach_answering<A, W, R>(name: &str, answer: A, work: W, refuse: R)
where
    A: Send + 'static,
    W: FnOnce(A) + Send + 'static,
    R: FnOnce(A, io::Error),
{
    let slot = Arc::new(Mutex::new(Some(answer)));
    let theirs = Arc::clone(&slot);
    let started = detach(name, move || {
        let answer = theirs.lock_ignoring_poison().take();
        if let Some(answer) = answer {
            work(answer);
        }
    });
    if let Err(unstarted) = started {
        let answer = slot.lock_ignoring_poison().take();
        if let Some(answer) = answer {
            refuse(answer, unstarted.error);
        }
    }
}

fn run_unstarted<W: FnOnce()>(name: &str, unstarted: Unstarted<W>) {
    crate::diag::report(&format!("thread {name}"), &unstarted.error);
    (unstarted.work)();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_reaches_the_work() {
        let (tx, rx) = std::sync::mpsc::channel();
        detach_answering(
            "answering-test",
            tx,
            |tx| tx.send(Ok(1)).unwrap(),
            |tx, e| tx.send(Err(e.kind())).unwrap(),
        );
        assert_eq!(rx.recv().unwrap(), Ok(1));
    }

    #[test]
    fn work_runs_on_its_own_named_thread() {
        let (tx, rx) = std::sync::mpsc::channel();
        detach("detached-test", move || {
            tx.send(std::thread::current().name().map(str::to_owned)).unwrap();
        })
        .unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("detached-test"));
    }

    fn refuse(_: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::OutOfMemory))
    }

    #[test]
    fn when_no_thread_starts_the_work_comes_back_unrun() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let unstarted = detach_with(move || tx.send(()).unwrap(), refuse).unwrap_err();
        assert_eq!(unstarted.error.kind(), io::ErrorKind::OutOfMemory);
        assert!(rx.try_recv().is_err());
        (unstarted.work)();
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn unstarted_work_can_run_here() {
        let here = std::thread::current().id();
        let (tx, rx) = std::sync::mpsc::channel();
        let unstarted = detach_with(move || tx.send(std::thread::current().id()).unwrap(), refuse).unwrap_err();
        run_unstarted("test", unstarted);
        assert_eq!(rx.recv().unwrap(), here);
    }
}
