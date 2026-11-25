//! The self-test's hooks of `status.rs`.

use super::*;

impl StatusLine {
    /// The texts the status line shows, for the self-test.
    pub(crate) fn texts(&self) -> Vec<String> {
        [&self.context, &self.cost, &self.activity, &self.problem]
            .iter()
            .map(|l| l.stringValue().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}
