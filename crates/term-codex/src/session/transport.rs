//! Bounded writes and outstanding requests for the app-server's stdio transport.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Sender, TrySendError, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{Reply, SessionError, Shared, reply};
use crate::wire::{Id, Message};

pub(super) const WRITE_QUEUE: usize = 256;
const REQUEST_LIMIT: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

type ResultValue = Result<Value, SessionError>;
pub(super) type After = Box<dyn FnOnce(&ResultValue, &Arc<Shared>) + Send>;

pub(super) struct Pending {
    reply: Sender<ResultValue>,
    after: Option<After>,
}

impl Pending {
    fn complete(self, result: ResultValue, shared: &Arc<Shared>) {
        if let Some(after) = self.after {
            after(&result, shared);
        }
        let _ = self.reply.send(result);
    }
}

impl Shared {
    pub(super) fn write(&self, message: &Message) -> Result<(), SessionError> {
        let mut line = message.encode().into_bytes();
        line.push(b'\n');
        let writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let writer = writer.as_ref().ok_or(SessionError::Closed)?;
        writer.try_send(line).map_err(|error| match error {
            TrySendError::Full(_) => SessionError::Congested,
            TrySendError::Disconnected(_) => SessionError::Closed,
        })
    }

    pub(super) fn notify(&self, method: &str, params: Value) -> Result<(), SessionError> {
        self.write(&Message::Notification {
            method: method.into(),
            params,
        })
    }

    pub(super) fn respond(&self, id: Id, result: Value) -> Result<(), SessionError> {
        self.write(&Message::Response { id, result })
    }

    /// Registers a request; a response completes it before the waiting caller is notified.
    /// Refusals and timeouts also complete it, including calls whose reply handle has been dropped.
    pub(super) fn call<T>(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        after: Option<After>,
        read: reply::Read<T>,
    ) -> Reply<T> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        let request = Message::Request {
            id: Id::Number(id),
            method: method.into(),
            params,
        };
        let call = Pending { reply: tx, after };
        // Hold the register until insertion so even an immediate server response finds its request.
        let mut pending = self.pending();
        let queued = if self.ended.load(Ordering::SeqCst) || self.stopping.load(Ordering::SeqCst) {
            Err(SessionError::Closed)
        } else if pending.len() >= REQUEST_LIMIT {
            Err(SessionError::Congested)
        } else {
            self.write(&request)
        };
        let refused = match queued {
            Ok(()) => {
                pending.insert(id, call);
                drop(pending);
                let shared = Arc::downgrade(self);
                term_base::timer::after(REQUEST_TIMEOUT, move || {
                    if let Some(shared) = shared.upgrade() {
                        shared.answered(id, Err(SessionError::Timeout));
                    }
                });
                None
            }
            Err(error) => {
                drop(pending);
                call.complete(Err(error.clone()), self);
                Some(error)
            }
        };
        Reply {
            rx,
            refused,
            id,
            deadline,
            shared: Arc::downgrade(self),
            read,
        }
    }

    pub(super) fn answered(self: &Arc<Self>, id: i64, result: ResultValue) {
        let call = self.pending().remove(&id);
        if let Some(call) = call {
            call.complete(result, self);
        }
    }
}
