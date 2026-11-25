//! The answer to a request of the client, once the server sends it.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::SessionError;
use crate::http::READ_TIMEOUT;

/// How a result of the wire turns into what the caller gets.
pub(crate) type Read<T> = fn(Value) -> Result<T, SessionError>;

/// A result that carries nothing.
pub(crate) fn unit(_: Value) -> Result<(), SessionError> {
    Ok(())
}

/// The result as it came.
pub(crate) fn raw(value: Value) -> Result<Value, SessionError> {
    Ok(value)
}

/// The answer to a request, once it comes; dropping it leaves the request running.
#[must_use = "a reply does nothing unless it is waited for or handed a callback"]
pub struct Reply<T> {
    rx: Receiver<Result<Value, SessionError>>,
    deadline: Instant,
    read: Read<T>,
}

impl<T> Reply<T> {
    /// A reply and the sender its request answers on.
    pub(crate) fn channel(read: Read<T>) -> (Sender<Result<Value, SessionError>>, Reply<T>) {
        let (tx, rx) = channel();
        let reply = Reply {
            rx,
            deadline: Instant::now() + READ_TIMEOUT + Duration::from_secs(5),
            read,
        };
        (tx, reply)
    }

    /// A reply that already failed with `error`.
    pub(crate) fn failed(error: SessionError, read: Read<T>) -> Reply<T> {
        let (tx, reply) = Reply::channel(read);
        drop(tx.send(Err(error)));
        reply
    }
}

impl<T: Send + 'static> Reply<T> {
    /// Waits for the answer, at most until the request's timeout.
    pub fn wait(self) -> Result<T, SessionError> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(Ok(result)) => (self.read)(result),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => Err(SessionError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(SessionError::Closed),
        }
    }

    /// Waits on a thread of its own and hands the answer to `f` there.
    pub fn then(self, f: impl FnOnce(Result<T, SessionError>) + Send + 'static) {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(f)));
        let theirs = slot.clone();
        let spawned = std::thread::Builder::new()
            .name("opencode-reply".into())
            .spawn(move || {
                let answer = self.wait();
                if let Some(f) = theirs.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    f(answer);
                }
            });
        if let Err(error) = spawned
            && let Some(f) = slot.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            f(Err(SessionError::Start(error.to_string())));
        }
    }
}
