//! The self-test's hooks of `banner.rs`.

use super::*;

impl Banner {
    pub(crate) fn shown(&self) -> Option<String> {
        (!self.root.isHidden()).then(|| self.message.stringValue().to_string())
    }
}
