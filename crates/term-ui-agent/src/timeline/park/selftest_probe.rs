//! The self-test's hooks of `park.rs`.

use super::*;

impl Timeline {
    /// Lets go of the cards as if the timeline had stayed out of view long enough.
    pub fn park_now(&self) {
        self.park();
    }

    /// How many rows the table shows, none while the timeline let go of its cards.
    pub fn rows_in_table(&self) -> usize {
        usize::try_from(self.table.numberOfRows()).unwrap_or(0)
    }
}
