//! The plan's usage, which the status line of Claude's tabs shows: from `get_usage`, which the CLI marks experimental,
//! else from the limits the session reported. The report is the account's, so one answer serves every tab.

use std::cell::{Cell, RefCell};
use std::time::{Duration, SystemTime};

use term_claude::Session;
use term_claude::control::UsageReport;
use term_thread::Thread;

use crate::common::util::main_soon;

/// How old a usage report may get before it is asked for again.
const FRESH: Duration = Duration::from_secs(120);

/// The latest answer to `get_usage` with when it came, and whether a question is out.
#[derive(Default)]
pub(crate) struct Usage {
    report: RefCell<Option<(SystemTime, Option<UsageReport>)>>,
    asking: Cell<bool>,
}

/// A window of the plan: the CLI's name for it, the share used from 0 to 1, and when it starts over.
pub(crate) struct Workspace {
    pub(crate) name: String,
    pub(crate) used: f64,
    pub(crate) resets: Option<String>,
}

/// The plan's weekly and five-hour window, in that order and where known: from the latest report, else from what
/// `thread`'s session reported.
pub(crate) fn windows(thread: &Thread) -> Vec<Workspace> {
    let cx = crate::host::state();
    let report = cx.usage.report.borrow().as_ref().and_then(|(_, r)| r.clone());
    windows_of(thread, report)
}

/// `windows` with `report` as the latest answer to `get_usage`.
fn windows_of(thread: &Thread, report: Option<UsageReport>) -> Vec<Workspace> {
    let report = report.filter(|_| thread.agent() == term_claude::AGENT);
    let mut windows: Vec<Workspace> = match report.filter(|r| !r.windows.is_empty()) {
        Some(report) => report
            .windows
            .iter()
            .map(|w| Workspace {
                name: w.name.clone(),
                used: w.utilization.unwrap_or(0.0) / 100.0,
                resets: w.resets_at.as_deref().map(super::iso_time),
            })
            .collect(),
        None => thread
            .rate_limits()
            .iter()
            .map(|l| Workspace {
                name: l.window.clone(),
                used: l.utilization.unwrap_or(0.0),
                resets: l.resets_at.map(super::time_of_day),
            })
            .collect(),
    };
    windows.retain(|w| matches!(w.name.as_str(), "seven_day" | "five_hour"));
    windows.sort_by_key(|w| w.name != "seven_day");
    windows
}

/// Asks `session` for the plan's usage when the last answer is old; the answer redraws the status line of every tab.
pub(crate) fn fetch(session: &Session) {
    let cx = crate::host::state();
    let usage = &cx.usage;
    let stale = usage
        .report
        .borrow()
        .as_ref()
        .is_none_or(|(at, _)| at.elapsed().unwrap_or(FRESH) >= FRESH);
    if !stale || usage.asking.replace(true) {
        return;
    }
    session.usage().then(move |result| {
        let report = result.ok();
        main_soon(move || {
            let cx = crate::host::state();
            *cx.usage.report.borrow_mut() = Some((SystemTime::now(), report));
            cx.usage.asking.set(false);
            let tabs = cx.agent_tabs.tabs.borrow().clone();
            for tab in tabs {
                tab.refresh();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use term_thread::{LimitStatus, RateLimit, ThreadId, Update};

    use super::*;

    #[test]
    fn a_claude_account_report_cannot_override_an_acp_threads_limits() {
        let report = UsageReport {
            windows: vec![term_claude::control::UsageWindow {
                name: "five_hour".into(),
                utilization: Some(80.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut acp = Thread::new(ThreadId::from("acp"), "codex-acp", "/work");
        acp.apply(Update::RateLimit(RateLimit {
            window: "five_hour".into(),
            status: LimitStatus::Allowed,
            utilization: Some(0.2),
            resets_at: None,
        }));
        assert_eq!(windows_of(&acp, Some(report.clone()))[0].used, 0.2);
        let claude = Thread::new(ThreadId::from("claude"), term_claude::AGENT, "/work");
        assert_eq!(windows_of(&claude, Some(report))[0].used, 0.8);
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;
