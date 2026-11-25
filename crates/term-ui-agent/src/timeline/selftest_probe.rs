//! The self-test's hooks of `timeline.rs`.

use super::*;

impl Timeline {
    pub fn table(&self) -> &NSTableView {
        &self.table
    }
}
