//! Looks at an open editor's file on the disk lane, so that a file the watcher reports, or one on a volume that stops
//! answering, never holds up typing; only what the look found comes back to the main thread.

use std::io;

use objc2::DefinedClass;
use term_document::{DiskCheck, DiskProbe};

use super::view::EditorTextView;
use crate::util::main_soon;

/// Why the file is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    /// The watcher or a git operation of the app's own reported it.
    Changed,
    /// The editor's text came back after a crash, and its copy is stored again after the look.
    Recovered,
}

/// Looks at the file of `view`, if it has one, and follows what changed there once the look is done.
pub(super) fn look(view: &EditorTextView, why: Look) {
    let Some(probe) = view.document().disk_probe() else {
        return;
    };
    let asked = view.ivars().file.disk_asked.get() + 1;
    view.ivars().file.disk_asked.set(asked);
    let serial = view.ivars().serial;
    let (sent, kept) = (probe.clone(), probe.clone());
    let queued = term_base::lane::DISK.ask(
        move || sent.check(),
        move |check| main_soon(move || found(serial, asked, &probe, why, check)),
    );
    if queued.is_err() {
        follow(view, why, Some(kept.check()));
    }
}

fn found(
    serial: crate::text_view::EditorSerial,
    asked: u64,
    probe: &DiskProbe,
    why: Look,
    check: Option<io::Result<DiskCheck>>,
) {
    let Some(view) = super::find_editor(serial) else {
        return;
    };
    if view.ivars().file.disk_asked.get() != asked {
        if why == Look::Recovered {
            super::recovery::store(&view);
        }
        return;
    }
    // A save or an earlier look moved the document's idea of its file since, which this look did not start from.
    if view.document().disk_probe().as_ref() != Some(probe) {
        look(&view, why);
        return;
    }
    follow(&view, why, check);
}

fn follow(view: &EditorTextView, why: Look, check: Option<io::Result<DiskCheck>>) {
    match check {
        Some(Ok(check)) => super::conflict::follow(view, check),
        Some(Err(error)) if why == Look::Recovered => term_base::diag::report("looking at a recovered file", error),
        _ => {}
    }
    if why == Look::Recovered {
        super::recovery::store(view);
    }
}
