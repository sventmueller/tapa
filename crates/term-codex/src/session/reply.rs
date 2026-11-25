//! The answer to a request of the client, once the server sends it.

use std::sync::Weak;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{SessionError, Shared};
use crate::wire::RpcError;

/// How a result of the wire turns into what the caller gets.
pub(crate) type Read<T> = fn(Value) -> Result<T, SessionError>;

/// The result as the type `T` the protocol describes.
pub(crate) fn typed<T: DeserializeOwned>(value: Value) -> Result<T, SessionError> {
    serde_json::from_value(value).map_err(|e| SessionError::Malformed(e.to_string()))
}

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
    pub(super) rx: Receiver<Result<Value, RpcError>>,
    pub(super) id: i64,
    pub(super) deadline: Instant,
    pub(super) shared: Weak<Shared>,
    pub(super) read: Read<T>,
}

impl<T: Send + 'static> Reply<T> {
    /// Waits for the answer, at most until the request's timeout.
    pub fn wait(self) -> Result<T, SessionError> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        match self.rx.recv_timeout(left) {
            Ok(Ok(result)) => (self.read)(result),
            Ok(Err(error)) => Err(SessionError::Server(error)),
            Err(RecvTimeoutError::Timeout) => {
                if let Some(shared) = self.shared.upgrade() {
                    shared.forget(self.id);
                }
                Err(SessionError::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(SessionError::Closed),
        }
    }

    /// Waits on a thread of its own and hands the answer to `f` there.
    pub fn then(self, f: impl FnOnce(Result<T, SessionError>) + Send + 'static) {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(f)));
        let theirs = slot.clone();
        let spawned = std::thread::Builder::new().name("codex-reply".into()).spawn(move || {
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

    /// The request's id on the wire.
    pub fn id(&self) -> i64 {
        self.id
    }
}
