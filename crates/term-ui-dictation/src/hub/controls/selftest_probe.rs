//! The self-test's hooks of `controls.rs`.

use super::*;

impl Mic {
    pub(crate) fn spinning(&self) -> bool {
        !self.spinner.isHidden()
    }
}
