//! The self-test's hooks of `notice.rs`.

use super::*;

impl Notice {
    pub(crate) fn text(&self) -> Option<String> {
        (!self.view.isHidden()).then(|| self.text.stringValue().to_string())
    }
}
