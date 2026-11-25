//! Copies of unsaved text against a crash (E24): written on a lane of their own once typing rests, and a few seconds into
//! steady typing at the latest, and removed once the text is saved or given up. They are sealed with the key the app
//! hands over (E17); without one no copy is written.

use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use objc2::DefinedClass;
use term_base::lane::Lane;
use term_document::recovery::{Debounce, Recovery, RecoveryId};

use super::view::EditorTextView;
use crate::util::main_after;

/// Writes the copies into the app's own data folder, apart from the disk lane, where a volume that stopped answering
/// may hold up every job behind it for minutes.
static COPIES: Lane = Lane::with_start("term-recovery", 1, 64, term_base::priority::background);

thread_local! {
    /// The copies' folder and key, once the host gave a key.
    static SEALED: std::cell::OnceCell<Recovery> = const { std::cell::OnceCell::new() };
}

/// Where the copies go and the key they are sealed with; `None` while the app has no key to give.
pub(crate) fn copies() -> Option<Recovery> {
    SEALED.with(|sealed| {
        if let Some(known) = sealed.get() {
            return Some(known.clone());
        }
        let host = crate::host::installed()?;
        let found = Recovery::new(host.recovery_dir(), host.recovery_key()?);
        Some(sealed.get_or_init(|| found).clone())
    })
}

/// How long typing rests before a copy is written.
const QUIET: Duration = Duration::from_secs(2);
/// How long steady typing goes on before a copy is written all the same.
const LONGEST: Duration = Duration::from_secs(10);

/// The copies of unsaved text that are due and when the timer that writes them fires.
pub(crate) struct RecoveryCopies {
    due: RefCell<Debounce>,
    /// When the timer that writes the copies due fires, if one is set.
    timer: Cell<Option<Instant>>,
}

impl Default for RecoveryCopies {
    fn default() -> Self {
        RecoveryCopies {
            due: RefCell::new(Debounce::new(QUIET, LONGEST)),
            timer: Cell::new(None),
        }
    }
}

fn with_due<R>(f: impl FnOnce(&RefCell<Debounce>) -> R) -> R {
    f(&crate::state().recovery_copies.due)
}

/// The view's text changed; its copy is written once the change is due.
pub(crate) fn changed(view: &EditorTextView) {
    let now = Instant::now();
    with_due(|due| due.borrow_mut().changed(&view.ivars().file.recovery, now));
    arm();
}

fn arm() {
    let Some(next) = with_due(|due| due.borrow().next_due()) else {
        return;
    };
    let state = crate::state();
    let timer = &state.recovery_copies.timer;
    if timer.get().is_some_and(|set| set <= next) {
        return;
    }
    timer.set(Some(next));
    main_after(next.saturating_duration_since(Instant::now()), fire);
}

fn fire() {
    crate::state().recovery_copies.timer.set(None);
    let due = with_due(|due| due.borrow_mut().due(Instant::now()));
    if !due.is_empty() {
        for view in super::open_editors() {
            if due.contains(&view.ivars().file.recovery) {
                store(&view);
            }
        }
    }
    arm();
}

/// Writes a copy of the view's text if it has unsaved changes, else drops the copy; the writing happens on the copies'
/// lane, in the order asked for.
pub(crate) fn store(view: &EditorTextView) {
    let id = view.ivars().file.recovery.clone();
    let document = view.document();
    if !document.is_dirty() {
        drop(document);
        drop_copy(id);
        return;
    }
    let Some(copies) = copies() else { return };
    let snapshot = document.snapshot();
    let title = document.path().map(|path| path.display().to_string());
    drop(document);
    let project = view.controller().and_then(|controller| controller.project());
    let job = {
        let id = id.clone();
        move || {
            let stored = copies.store(&id, &snapshot, title.as_deref(), project.as_deref());
            if let Err(error) = stored {
                term_base::diag::report("recovery copy", error);
            }
        }
    };
    if COPIES.try_run(job).is_err() {
        with_due(|due| due.borrow_mut().changed(&id, Instant::now()));
        arm();
    }
}

/// Drops the copy of a view that was saved or closed.
pub(crate) fn discard(view: &EditorTextView) {
    drop_copy(view.ivars().file.recovery.clone());
}

fn drop_copy(id: RecoveryId) {
    with_due(|due| due.borrow_mut().forget(&id));
    let Some(copies) = copies() else { return };
    let job = {
        let (id, copies) = (id.clone(), copies.clone());
        move || {
            if let Err(error) = copies.discard(&id) {
                term_base::diag::report("recovery copy", error);
            }
        }
    };
    if COPIES.try_run(job).is_err()
        && let Err(error) = copies.discard(&id)
    {
        term_base::diag::report("recovery copy", error);
    }
}

/// What a quit waits for of the copies: the copies' lane to have written and removed what it was asked to.
pub struct QuitCopies {
    ids: Vec<RecoveryId>,
    copies: Option<Recovery>,
    finished: Option<mpsc::Receiver<()>>,
}

/// At a quit the user agreed to: drops the copies of every open editor, whose text is saved or given up; what to wait
/// for before the app ends.
pub(crate) fn quit() -> QuitCopies {
    let ids: Vec<RecoveryId> = super::open_editors()
        .iter()
        .map(|view| view.ivars().file.recovery.clone())
        .collect();
    for id in &ids {
        drop_copy(id.clone());
    }
    let (done, finished) = mpsc::channel();
    let queued = COPIES.try_run(move || {
        let _ = done.send(());
    });
    QuitCopies {
        ids,
        copies: copies(),
        finished: queued.is_ok().then_some(finished),
    }
}

impl QuitCopies {
    /// Waits up to `limit` for the lane, on any thread; removals it has not reached by then are done here instead,
    /// since a copy left behind would come back at the next launch.
    pub fn finish(self, limit: Duration) {
        if self
            .finished
            .is_some_and(|finished| finished.recv_timeout(limit).is_ok())
        {
            return;
        }
        term_base::diag::report("recovery copy", "the lane did not finish before the quit");
        let Some(copies) = &self.copies else { return };
        for id in &self.ids {
            if let Err(error) = copies.discard(id) {
                term_base::diag::report("recovery copy", error);
            }
        }
    }
}
