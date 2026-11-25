//! The self-test's hooks of `model.rs`.

use super::*;

impl State {
    /// Whether a bell rang since the view opened.
    pub(crate) fn has_rung(&self) -> bool {
        self.last_bell.get().is_some()
    }
}
