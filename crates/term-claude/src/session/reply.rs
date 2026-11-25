//! The answer to a control request: a slot that the session's reader, the timer or the session's end fills once, and
//! the `Reply` that waits for it or hands it to a callback, without a thread of its own.

use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde_json::Value;
use term_base::{IgnoringPoison, LockIgnoringPoison};

use crate::control::{Answer, ControlError};

type Outcome = Result<Value, ControlError>;
type Then = Box<dyn FnOnce(Outcome) + Send>;

enum Stage {
    Waiting,
    Answered(Outcome),
    Then(Then),
    Done,
}

struct Slot {
    stage: Mutex<Stage>,
    answered: Condvar,
}

impl Slot {
    fn fill(&self, outcome: Outcome) {
        let then = {
            let mut stage = self.stage.lock_ignoring_poison();
            match std::mem::replace(&mut *stage, Stage::Done) {
                Stage::Waiting => {
                    *stage = Stage::Answered(outcome);
                    self.answered.notify_all();
                    return;
                }
                Stage::Then(then) => then,
                earlier @ (Stage::Answered(_) | Stage::Done) => {
                    *stage = earlier;
                    return;
                }
            }
        };
        term_base::lane::contained(|| then(outcome));
    }
}

/// The session's end of a control request; dropped unanswered, it answers that the CLI ended.
pub(super) struct Answering(Option<Arc<Slot>>);

impl Answering {
    /// Answers the request with `outcome`, calling a callback waiting for it on this thread.
    pub(super) fn answer(mut self, outcome: Outcome) {
        if let Some(slot) = self.0.take() {
            slot.fill(outcome);
        }
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        if let Some(slot) = self.0.take() {
            slot.fill(Err(ControlError::Closed));
        }
    }
}

/// The answer to a control request, once it comes; dropping it leaves the request running until it is answered or
/// times out.
pub struct Reply<T> {
    slot: Arc<Slot>,
    answer: PhantomData<fn() -> T>,
}

/// The two ends of a request.
pub(super) fn pair<T>() -> (Answering, Reply<T>) {
    let slot = Arc::new(Slot {
        stage: Mutex::new(Stage::Waiting),
        answered: Condvar::new(),
    });
    let reply = Reply {
        slot: Arc::clone(&slot),
        answer: PhantomData,
    };
    (Answering(Some(slot)), reply)
}

impl<T: Answer> Reply<T> {
    /// Waits for the answer; it comes at the latest when the request times out and the session withdraws it.
    pub fn wait(self) -> Result<T, ControlError> {
        let mut stage = self.slot.stage.lock_ignoring_poison();
        loop {
            match std::mem::replace(&mut *stage, Stage::Done) {
                Stage::Answered(outcome) => return outcome.and_then(T::read),
                Stage::Waiting => {
                    *stage = Stage::Waiting;
                    stage = self.slot.answered.wait(stage).ignoring_poison();
                }
                Stage::Then(_) | Stage::Done => return Err(ControlError::Closed),
            }
        }
    }

    /// Hands the answer to `f` without a thread of its own: on the session's reader thread when the CLI answers, on
    /// the app's timer thread when the request times out or was answered already. `f` must not block, as the
    /// session's frames or the app's deadlines wait meanwhile; a panic in it is contained.
    pub fn then(self, f: impl FnOnce(Result<T, ControlError>) + Send + 'static) {
        let then: Then = Box::new(move |outcome| f(outcome.and_then(T::read)));
        let mut stage = self.slot.stage.lock_ignoring_poison();
        match std::mem::replace(&mut *stage, Stage::Done) {
            Stage::Waiting => *stage = Stage::Then(then),
            Stage::Answered(outcome) => {
                drop(stage);
                term_base::timer::after(Duration::ZERO, move || then(outcome));
            }
            Stage::Then(_) | Stage::Done => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    #[test]
    fn a_callback_hears_the_answer_on_the_answering_thread() {
        let (answering, reply) = pair::<()>();
        let (tx, rx) = channel();
        reply.then(move |result| tx.send((result, std::thread::current().id())).unwrap());
        assert!(rx.try_recv().is_err());
        answering.answer(Ok(serde_json::json!({})));
        assert_eq!(rx.try_recv().unwrap(), (Ok(()), std::thread::current().id()));
    }

    #[test]
    fn an_answer_that_came_first_reaches_a_later_callback_off_the_calling_thread() {
        let (answering, reply) = pair::<()>();
        answering.answer(Err(ControlError::Timeout));
        let (tx, rx) = channel();
        reply.then(move |result| tx.send((result, std::thread::current().id())).unwrap());
        let (result, thread) = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(result, Err(ControlError::Timeout));
        assert_ne!(thread, std::thread::current().id());
    }

    #[test]
    fn a_request_dropped_by_the_session_answers_closed_and_only_once() {
        let (answering, reply) = pair::<()>();
        let waiter = std::thread::spawn(move || reply.wait());
        drop(answering);
        assert_eq!(waiter.join().unwrap(), Err(ControlError::Closed));
        let (answering, reply) = pair::<()>();
        let slot = Arc::clone(answering.0.as_ref().unwrap());
        answering.answer(Err(ControlError::Timeout));
        slot.fill(Ok(serde_json::json!({})));
        assert_eq!(reply.wait(), Err(ControlError::Timeout));
    }

    #[test]
    fn a_panicking_callback_is_contained() {
        let (answering, reply) = pair::<()>();
        reply.then(|_| panic!("a callback that panics"));
        answering.answer(Ok(serde_json::json!({})));
    }
}
