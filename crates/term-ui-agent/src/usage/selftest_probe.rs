//! The self-test's hooks of `usage.rs`.

use super::*;

/// Takes `report` as the latest answer to `get_usage`, as the self-test hands one over without a session.
pub fn inject(report: UsageReport) {
    *crate::host::state().usage.report.borrow_mut() = Some((SystemTime::now(), Some(report)));
    for tab in super::super::all() {
        tab.refresh();
    }
}
