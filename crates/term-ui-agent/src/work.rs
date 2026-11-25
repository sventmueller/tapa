//! Which of a session's work the lists of its head show, and how much of it runs.

use splat_i18n::t;
use term_thread::{BackgroundKind, TaskStatus, Thread};

/// Which of a session's work a list shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// Its subagents and the background work that runs, as `/tasks` shows them.
    All,
    /// Its subagents, with the workflows that start them.
    Subagents,
    /// Its shell commands and monitors.
    Background,
}

impl Part {
    /// Whether a background item of `kind` is in the part's list.
    pub fn holds(self, kind: &BackgroundKind) -> bool {
        match self {
            Part::All => *kind != BackgroundKind::Subagent,
            Part::Subagents => *kind == BackgroundKind::Workflow,
            Part::Background => !matches!(kind, BackgroundKind::Subagent | BackgroundKind::Workflow),
        }
    }

    /// The title above the part's list.
    pub fn heading(self) -> &'static str {
        match self {
            Part::All => t("Aufgaben dieser Sitzung"),
            Part::Subagents => t("Subagenten dieser Sitzung"),
            Part::Background => t("Hintergrundprozesse dieser Sitzung"),
        }
    }
}

/// How many entries of a part run and how many ended, for the button in the session's head.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Tally {
    /// What runs, without the housekeeping that activity indicators leave out.
    pub(crate) running: usize,
    pub(crate) ended: usize,
    /// Every entry the list shows.
    pub(crate) listed: usize,
}

/// What the list of `part` of `thread` holds.
pub(crate) fn tally(thread: &Thread, part: Part) -> Tally {
    let mut tally = Tally::default();
    let mut count = |status: TaskStatus, ambient: bool| {
        tally.listed += 1;
        if status.is_final() {
            tally.ended += 1;
        } else if !ambient {
            tally.running += 1;
        }
    };
    if part != Part::Background {
        for sub in thread.subagents() {
            count(sub.status, false);
        }
    }
    for item in thread.background().iter().filter(|b| part.holds(&b.kind)) {
        count(item.status, item.ambient);
    }
    tally
}
