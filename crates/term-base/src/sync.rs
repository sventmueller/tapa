//! Locks that keep working after a thread panicked while holding them: the data behind them is plain state the next
//! user can read, and the panic itself is contained and reported where it happened.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// A mutex taken whether or not an earlier holder panicked.
pub trait LockIgnoringPoison<T: ?Sized> {
    /// The guard, poisoned or not.
    fn lock_ignoring_poison(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockIgnoringPoison<T> for Mutex<T> {
    fn lock_ignoring_poison(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The value of any other poisonable result — a condition variable's wait, a read or write lock, a mutex taken
/// apart — whether or not it is poisoned.
pub trait IgnoringPoison<T> {
    /// The value, poisoned or not.
    fn ignoring_poison(self) -> T;
}

impl<T> IgnoringPoison<T> for Result<T, PoisonError<T>> {
    fn ignoring_poison(self) -> T {
        self.unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    #[test]
    fn a_poisoned_mutex_still_hands_out_its_value() {
        let shared = Arc::new(Mutex::new(1));
        let held = Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            let _guard = held.lock();
            panic!("poison it");
        })
        .join();
        assert!(shared.is_poisoned());
        *shared.lock_ignoring_poison() += 1;
        assert_eq!(*shared.lock_ignoring_poison(), 2);
    }

    #[test]
    fn a_poisoned_rwlock_still_reads() {
        let shared = Arc::new(RwLock::new(3));
        let held = Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            let _guard = held.write();
            panic!("poison it");
        })
        .join();
        assert_eq!(*shared.read().ignoring_poison(), 3);
    }
}
