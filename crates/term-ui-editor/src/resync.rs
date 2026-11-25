//! Putting an editor back in step after a bug in one of its AppKit callbacks panicked (Review B9). The panic is caught
//! so that it does not unwind through AppKit, but it may have stopped half way through following an edit: the
//! document changed, and the colours or the unsaved marker did not.

use std::cell::Cell;

use term_base::lane::contained;

/// What putting an editor back in step does, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The view shows the document's text again.
    ShowDocument,
    /// The colours are worked out for the whole text.
    Recolor,
    /// The tab shows whether the text is saved.
    ShowDirty,
    /// What else follows the text takes it from the start.
    Follow,
}

/// The steps for an editor whose view shows another text than its document when `drifted`.
pub(crate) fn steps(drifted: bool) -> Vec<Step> {
    let mut steps = Vec::with_capacity(4);
    if drifted {
        steps.push(Step::ShowDocument);
    }
    steps.extend([Step::Recolor, Step::ShowDirty, Step::Follow]);
    steps
}

/// Whether an editor waits to be put back in step, and whether that runs now.
#[derive(Debug, Default)]
pub(crate) struct Recovery {
    due: Cell<bool>,
    running: Cell<bool>,
}

impl Recovery {
    /// Runs `f`, a callback from AppKit, or gives `fallback` when it panics, and then has `ask` request a resync on a
    /// later turn of the run loop.
    pub(crate) fn guarded<R>(&self, fallback: R, f: impl FnOnce() -> R, ask: impl FnOnce()) -> R {
        match contained(f) {
            Some(result) => result,
            None => {
                self.ask(ask);
                fallback
            }
        }
    }

    /// Has `ask` request a resync, once for any number of requests until it runs; a request while it runs is left out,
    /// so a step that always panics does not run again and again.
    pub(crate) fn ask(&self, ask: impl FnOnce()) {
        if !self.running.get() && !self.due.replace(true) {
            ask();
        }
    }

    /// Puts the editor back in step if a request is due: `run` makes each step for `drifted()`, a step that panics
    /// keeping none of the others from running. The steps that panicked.
    pub(crate) fn resync(&self, drifted: impl FnOnce() -> bool, run: impl Fn(Step)) -> Vec<Step> {
        if !self.due.replace(false) {
            return Vec::new();
        }
        self.running.set(true);
        let drifted = contained(drifted).unwrap_or(true);
        let failed = steps(drifted)
            .into_iter()
            .filter(|step| contained(|| run(*step)).is_none())
            .collect();
        self.running.set(false);
        failed
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    const ALWAYS: [Step; 3] = [Step::Recolor, Step::ShowDirty, Step::Follow];

    #[test]
    fn a_callback_that_returns_asks_for_nothing() {
        let recovery = Recovery::default();
        let asked = Cell::new(0);
        assert_eq!(recovery.guarded(0, || 7, || asked.set(asked.get() + 1)), 7);
        assert_eq!(asked.get(), 0);
        let ran = RefCell::new(vec![]);
        assert!(recovery.resync(|| true, |step| ran.borrow_mut().push(step)).is_empty());
        assert!(ran.borrow().is_empty(), "nothing is due");
    }

    #[test]
    fn panics_give_the_fallback_and_ask_once_until_the_resync_runs() {
        let recovery = Recovery::default();
        let asked = Cell::new(0);
        let ask = || asked.set(asked.get() + 1);
        assert!(recovery.guarded(true, || panic!("in a callback"), ask));
        assert_eq!(recovery.guarded(3, || -> i32 { panic!("again") }, ask), 3);
        recovery.ask(ask);
        assert_eq!(asked.get(), 1);
        recovery.resync(|| false, |_| ());
        assert!(recovery.guarded(true, || panic!("after the resync"), ask));
        assert_eq!(asked.get(), 2, "a panic after the resync asks again");
    }

    #[test]
    fn the_document_goes_into_the_view_only_when_they_differ() {
        assert_eq!(steps(false), ALWAYS);
        let drifted = steps(true);
        assert_eq!(drifted[0], Step::ShowDocument);
        assert_eq!(drifted[1..], ALWAYS);
    }

    #[test]
    fn a_resync_makes_every_step_once() {
        let recovery = Recovery::default();
        recovery.ask(|| ());
        let ran = RefCell::new(vec![]);
        let failed = recovery.resync(|| true, |step| ran.borrow_mut().push(step));
        assert!(failed.is_empty());
        assert_eq!(*ran.borrow(), steps(true));
        assert!(recovery.resync(|| true, |step| ran.borrow_mut().push(step)).is_empty());
        assert_eq!(
            ran.borrow().len(),
            steps(true).len(),
            "the second resync had nothing due"
        );
    }

    #[test]
    fn a_step_that_panics_keeps_none_of_the_others_from_running() {
        let recovery = Recovery::default();
        recovery.ask(|| ());
        let ran = RefCell::new(vec![]);
        let failed = recovery.resync(
            || false,
            |step| {
                assert_ne!(step, Step::Recolor, "a bug in the colours");
                ran.borrow_mut().push(step);
            },
        );
        assert_eq!(failed, [Step::Recolor]);
        let wanted: Vec<Step> = ALWAYS.into_iter().filter(|s| *s != Step::Recolor).collect();
        assert_eq!(*ran.borrow(), wanted);
    }

    #[test]
    fn a_panic_while_the_resync_runs_asks_for_no_further_one() {
        let recovery = Recovery::default();
        let asked = Cell::new(0);
        recovery.ask(|| asked.set(asked.get() + 1));
        recovery.resync(
            || false,
            |_| {
                recovery.guarded(
                    (),
                    || panic!("in a callback the resync caused"),
                    || asked.set(asked.get() + 1),
                );
            },
        );
        assert_eq!(asked.get(), 1);
        let ran = Cell::new(false);
        recovery.resync(|| false, |_| ran.set(true));
        assert!(!ran.get(), "nothing was left due");
    }

    #[test]
    fn a_check_that_panics_counts_as_drifted() {
        let recovery = Recovery::default();
        recovery.ask(|| ());
        let ran = RefCell::new(vec![]);
        recovery.resync(|| panic!("comparing the texts"), |step| ran.borrow_mut().push(step));
        assert_eq!(ran.borrow().first(), Some(&Step::ShowDocument));
    }
}
