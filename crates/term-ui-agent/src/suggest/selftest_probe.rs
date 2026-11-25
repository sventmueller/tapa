//! The self-test's hooks of `suggest.rs`.

use super::*;

impl Suggestions {
    /// The labels shown, for the self-test.
    pub(crate) fn labels(&self) -> Vec<String> {
        self.items.borrow().iter().map(|i| i.label.clone()).collect()
    }
}
