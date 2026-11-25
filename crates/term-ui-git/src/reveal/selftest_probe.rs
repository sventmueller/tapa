//! The self-test's hooks of `reveal.rs`.

use super::*;
use crate::panel;

/// The hash of the commit selected in the Git panel of `controller`, for the self-test.
pub fn selected_commit(controller: &(impl AsWorkspace + ?Sized)) -> Option<CommitId> {
    let controller = &controller.as_workspace();
    let panel = panel::of(controller)?;
    let outline = &panel.ui().outline;
    let item = outline.itemAtRow(outline.selectedRow())?;
    match panel::key_of(&item)? {
        RowKey::Commit(hash) => Some(hash),
        _ => None,
    }
}
