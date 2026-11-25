//! What the self-test reads from and does with the stash of a workspace's Git panel, through the code its controls
//! run.

use std::path::{Path, PathBuf};

use objc2::runtime::Sel;
use objc2::sel;
use objc2_app_kit::NSAccessibility;

use super::model::RowKey;
use super::panel;
use term_ui::workspace_host::AsWorkspace;

/// A stash entry as its row shows it: commit, message.
pub(crate) type StashSeen = (term_git::CommitId, String);

/// The stash entries the panel lists, newest first.
pub fn stashes(controller: &(impl AsWorkspace + ?Sized)) -> Vec<StashSeen> {
    let controller = &controller.as_workspace();
    let Some(panel) = panel::of(controller) else {
        return Vec::new();
    };
    let state = panel.read();
    state
        .model
        .stashes
        .iter()
        .map(|s| (s.hash.clone(), s.message.clone()))
        .collect()
}

/// The count the section's pill shows, if the section shows.
pub fn stash_pill(controller: &(impl AsWorkspace + ?Sized)) -> Option<String> {
    let controller = &controller.as_workspace();
    let panel = panel::of(controller)?;
    let item = panel.item_for(&RowKey::Section(super::model::Section::Stashes));
    let outline = &panel.ui().outline;
    // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
    let row = unsafe { outline.rowForItem(Some(&item)) };
    let view = outline.viewAtColumn_row_makeIfNecessary(0, row, true)?;
    Some(view.accessibilityLabel()?.to_string())
}

/// Whether a stash apply waits at its conflicts, and whether it was a pop.
pub fn stash_conflict(controller: &(impl AsWorkspace + ?Sized)) -> Option<bool> {
    let controller = &controller.as_workspace();
    panel::of(controller)?.read().stash_apply.as_ref().map(|s| s.pop)
}

/// The banner's title, text and buttons, while it shows.
pub fn banner(controller: &(impl AsWorkspace + ?Sized)) -> Option<[String; 4]> {
    let controller = &controller.as_workspace();
    let panel = panel::of(controller)?;
    let banner = &panel.ui().banner;
    (!banner.view.isHidden()).then(|| {
        [
            banner.title.stringValue().to_string(),
            banner.detail.stringValue().to_string(),
            banner.resume.title().to_string(),
            banner.abort.title().to_string(),
        ]
    })
}

/// "Stash …" from the menu beside "Commit".
pub fn stash(controller: &(impl AsWorkspace + ?Sized)) {
    let controller = &controller.as_workspace();
    if let Some(panel) = panel::of(controller) {
        panel.ask_stash();
    }
}

/// "Nur Vorgemerktes stashen".
pub fn stash_staged(controller: &(impl AsWorkspace + ?Sized)) {
    let controller = &controller.as_workspace();
    if let Some(panel) = panel::of(controller) {
        panel.stash_staged();
    }
}

/// What a stash row offers.
#[derive(Debug, Clone, Copy)]
#[expect(missing_docs, reason = "the names say it")]
pub enum StashAction {
    Apply,
    Pop,
    Drop,
}

/// Runs `action` on the stash entry `hash`, as its button does.
pub fn stash_action(controller: &(impl AsWorkspace + ?Sized), hash: &term_git::CommitId, action: StashAction) {
    let controller = &controller.as_workspace();
    let selector = match action {
        StashAction::Apply => sel!(termGitApplyStash:),
        StashAction::Pop => sel!(termGitPopStash:),
        StashAction::Drop => sel!(termGitDropStash:),
    };
    if let Some(panel) = panel::of(controller) {
        panel.run_row_action(selector, vec![RowKey::Stash(hash.clone())]);
    }
}

/// "Fertig" or "Abbrechen …" in the banner of a stash conflict.
pub fn end_conflict(controller: &(impl AsWorkspace + ?Sized), finish: bool) {
    let controller = &controller.as_workspace();
    if let Some(panel) = panel::of(controller) {
        if finish {
            panel.finish_stash_conflict();
        } else {
            panel.abort_stash_conflict();
        }
    }
}

/// Opens the row of the stash entry `hash`, as a click on it does.
pub fn open_stash(controller: &(impl AsWorkspace + ?Sized), hash: &term_git::CommitId) {
    let controller = &controller.as_workspace();
    let Some(panel) = panel::of(controller) else { return };
    let item = panel.item_for(&RowKey::Stash(hash.clone()));
    // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
    let row = unsafe { panel.ui().outline.rowForItem(Some(&item)) };
    panel.preview_row(row);
}

/// Selects the row of the stash entry `hash`, and only it.
pub fn select_stash(controller: &(impl AsWorkspace + ?Sized), hash: &term_git::CommitId) {
    let controller = &controller.as_workspace();
    let Some(panel) = panel::of(controller) else { return };
    let outline = &panel.ui().outline;
    // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
    let row = unsafe { outline.rowForItem(Some(&panel.item_for(&RowKey::Stash(hash.clone())))) };
    if let Ok(row) = usize::try_from(row) {
        outline.selectRowIndexes_byExtendingSelection(&objc2_foundation::NSIndexSet::indexSetWithIndex(row), false);
    }
}

/// The files the list shows below the stash entry `hash`.
pub fn stash_rows(controller: &(impl AsWorkspace + ?Sized), hash: &term_git::CommitId) -> Vec<PathBuf> {
    let controller = &controller.as_workspace();
    let Some(panel) = panel::of(controller) else {
        return Vec::new();
    };
    let outline = &panel.ui().outline;
    (0..outline.numberOfRows())
        .filter_map(|row| outline.itemAtRow(row))
        .filter_map(|item| match panel::key_of(&item)? {
            RowKey::StashFile(at, file) if at == *hash => Some(file),
            _ => None,
        })
        .collect()
}

/// Shows what the stash entry `hash` changed in `file`, as a double click on the file's row does.
pub fn show_stash_diff(controller: &(impl AsWorkspace + ?Sized), hash: &term_git::CommitId, file: &Path) {
    let controller = &controller.as_workspace();
    if let Some(panel) = panel::of(controller) {
        panel.show_diff(&RowKey::StashFile(hash.clone(), file.to_path_buf()), false);
    }
}

/// Rows and hunks of the diff tab of what the stash entry `hash` changed in `file`.
pub fn stash_diff_counts(
    controller: &(impl AsWorkspace + ?Sized),
    hash: &term_git::CommitId,
    file: &Path,
) -> Option<(usize, usize)> {
    let controller = &controller.as_workspace();
    super::diff::find(controller, &super::diff::stash_key(hash, file)).map(|tab| tab.counts())
}

/// Whether the menu bar's Git command `action` can run now, on the selected rows.
pub fn can(controller: &(impl AsWorkspace + ?Sized), action: Sel) -> bool {
    let controller = &controller.as_workspace();
    panel::of(controller).is_some_and(|panel| panel.can(action, &panel.selected_keys()))
}
