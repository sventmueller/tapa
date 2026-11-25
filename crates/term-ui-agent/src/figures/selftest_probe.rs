//! The self-test's hooks of `figures.rs`.

use super::*;

/// Whether the answers asked for `key`, for the self-test.
pub fn asked_for(key: &Key) -> bool {
    with_asked(|asked| asked.borrow().as_ref().is_some_and(|asked| asked.keys.contains(key)))
}
