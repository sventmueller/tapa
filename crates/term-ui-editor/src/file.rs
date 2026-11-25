//! The file behind an editor as the editor follows it: whether its tab shows it unsaved, the save that writes it, the
//! looks at the disk, the copy against a crash, and what the bar above says about the file.

use std::cell::{Cell, RefCell};

use objc2::DefinedClass;
use term_document::recovery::RecoveryId;

use super::view::EditorTextView;
use super::{conflict, saving};

/// What an editor knows of its file beside the document.
pub(crate) struct File {
    /// The dirty state the tab shows, to update it only on a change.
    pub(super) shown_dirty: Cell<bool>,
    /// Whether a save writes its file on the disk lane.
    pub(super) writing: Cell<saving::Writing>,
    /// What hears of the running write once it is over.
    pub(super) after_write: RefCell<Vec<saving::Then>>,
    /// The saves that wait for the running write.
    pub(super) waiting: RefCell<Vec<saving::Waiting>>,
    /// The user gave up the unsaved text for a close that still waits for another editor's save.
    pub(super) given_up: Cell<bool>,
    /// Counts the looks at the file on disk, so that only the answer to the latest is followed.
    pub(super) disk_asked: Cell<u64>,
    /// Names the copy of the unsaved text against a crash, the one a restored view came from.
    pub(super) recovery: RecoveryId,
    /// What the bar above the editor is about: a change of the file on disk, conflict markers or a gone file.
    pub(super) conflict: RefCell<Option<conflict::State>>,
}

impl File {
    /// The state of a new editor whose copy against a crash is `recovery`.
    pub(super) fn new(recovery: RecoveryId) -> File {
        File {
            shown_dirty: Cell::new(false),
            writing: Cell::new(saving::Writing::Idle),
            after_write: RefCell::default(),
            waiting: RefCell::default(),
            given_up: Cell::new(false),
            disk_asked: Cell::new(0),
            recovery,
            conflict: RefCell::new(None),
        }
    }
}

/// Shows the dirty state of the view's document on its tab when it changed.
pub(crate) fn update_dirty(view: &EditorTextView) {
    let dirty = view.document().is_dirty();
    if dirty == view.ivars().file.shown_dirty.get() {
        return;
    }
    show_dirty(view, dirty);
}

/// Shows the dirty state of the view's document on its tab, whatever the tab showed before.
pub(super) fn show_dirty_anew(view: &EditorTextView) {
    let dirty = view.document().is_dirty();
    show_dirty(view, dirty);
}

fn show_dirty(view: &EditorTextView, dirty: bool) {
    view.ivars().file.shown_dirty.set(dirty);
    if let (Some(controller), Some(tab)) = (view.controller(), view.ivars().tab.get()) {
        crate::host::host().set_tab_dirty(controller.id(), tab, dirty);
        if dirty {
            crate::opening::keep_if_preview(controller.id(), tab);
        }
    }
}
