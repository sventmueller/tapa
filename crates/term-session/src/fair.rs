//! The lock the reader thread and the owner share the screen under.

use std::sync::{Mutex, MutexGuard};

use term_base::LockIgnoringPoison;

/// A mutex whose waiting thread gets the screen before the holder can take it again: the reader thread takes a
/// lease while it parses a batch, so a snapshot waits for that batch at most rather than for a stream of them.
pub(crate) struct FairMutex<T> {
    data: Mutex<T>,
    next: Mutex<()>,
}

impl<T> FairMutex<T> {
    pub(crate) fn new(data: T) -> FairMutex<T> {
        FairMutex {
            data: Mutex::new(data),
            next: Mutex::new(()),
        }
    }

    /// Reserves the lock: whoever calls `lock` meanwhile waits until the lease is dropped.
    pub(crate) fn lease(&self) -> MutexGuard<'_, ()> {
        self.next.lock_ignoring_poison()
    }

    /// Locks in turn behind a lease.
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        let _next = self.next.lock_ignoring_poison();
        self.data.lock_ignoring_poison()
    }

    /// Locks without waiting for a lease, for the holder of one.
    pub(crate) fn lock_unfair(&self) -> MutexGuard<'_, T> {
        self.data.lock_ignoring_poison()
    }
}
