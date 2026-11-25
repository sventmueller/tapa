//! What the self-test reads from and does with the parts of `failure.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

impl FailurePage {
    pub(crate) fn offers_proceed(&self) -> bool {
        !self.view.isHidden() && !self.proceed.isHidden()
    }

    pub(crate) fn is_shown(&self) -> bool {
        !self.view.isHidden()
    }

    pub(crate) fn heading(&self) -> String {
        self.heading.stringValue().to_string()
    }
}
