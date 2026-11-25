//! Adds entries at the end without building the indices of the earlier rows again.

use term_thread::{Change, Lane, Thread};

use super::{Timeline, hidden_in_focus, is_quiet_hook, is_running_hook};

impl Timeline {
    pub(super) fn append_rows(&self, thread: &Thread, changes: &[Change]) -> bool {
        let previous = self.thread.borrow().entries(&Lane::Main).len();
        let entries = thread.entries(&Lane::Main);
        let Some(tail) = entries.get(previous..).filter(|tail| !tail.is_empty()) else {
            return false;
        };
        let mut next = previous;
        for change in changes {
            match change {
                Change::Cleared | Change::Removed { lane: Lane::Main, .. } => return false,
                Change::Inserted {
                    lane: Lane::Main,
                    id,
                    index,
                } => {
                    if *index != next || entries.get(next).is_none_or(|entry| entry.id != *id) {
                        return false;
                    }
                    next += 1;
                }
                Change::Updated { lane: Lane::Main, id }
                    if thread.place(*id).is_none_or(|(_, index)| index < previous) =>
                {
                    return false;
                }
                _ => {}
            }
        }
        if next != entries.len() || tail.iter().any(|entry| is_quiet_hook(entry) || is_running_hook(entry)) {
            return false;
        }
        let focus = self.focus.get();
        let mut rows = self.rows.borrow_mut();
        let mut positions = self.positions.borrow_mut();
        let mut list = self.list.borrow_mut();
        for entry in tail.iter().filter(|entry| !focus || !hidden_in_focus(entry)) {
            positions.insert(entry.id, rows.len());
            rows.push(entry.id);
            list.push(entry.clone());
        }
        true
    }
}
