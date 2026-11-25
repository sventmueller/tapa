//! The self-test's hooks of `host.rs`.

use super::*;

/// The number of the terminal view opened last.
pub(crate) fn latest_id() -> Option<u64> {
    host().state().views.borrow().keys().max().copied()
}
