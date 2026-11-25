//! Which rows of the Git list show the same as when their cells were built, so the list hands those cells back
//! instead of building them again.

use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::model::RowKey;
use super::panel::GitPanel;

impl GitPanel {
    /// The cell of `key`'s row, the one built before while the row shows the same.
    pub(crate) fn kept_row_view(&self, key: &RowKey) -> Option<Retained<NSView>> {
        let Some(look) = self.look(key) else {
            return self.row_view(key);
        };
        self.cells().cell(key, &look, || self.row_view(key))
    }

    /// What the row of `key` shows, at the interface size; tooltips that are written when they come up are left
    /// out.
    fn look(&self, key: &RowKey) -> Option<(u64, String)> {
        let state = self.read();
        let model = &state.model;
        let shown = match key {
            RowKey::Section(section) => model.len(*section).to_string(),
            RowKey::File(section, path) => format!("{:?}", model.file(*section, path)?),
            RowKey::Commit(hash) => {
                let (commit, graph) = model.commit_row(hash)?;
                let age = super::text::ago(super::history::now() - commit.author_time);
                let remotes = super::history::remotes_at(&state, commit);
                format!(
                    "{:?} {:?} {:?} {remotes:?} {graph:?} {age}",
                    commit.subject, commit.author_name, commit.refs
                )
            }
            RowKey::CommitFile(hash, path) => {
                let lanes = model.commit_row(hash)?.1.through();
                let change = state
                    .commit_changes
                    .get(hash)?
                    .files
                    .iter()
                    .find(|f| f.path == *path)?
                    .change;
                format!("{lanes:?} {change:?}")
            }
            RowKey::More => format!("{:?}", model.last_lanes()),
            RowKey::Stash(hash) => {
                let stash = model.stash(hash)?;
                format!("{stash:?} {}", super::text::ago(super::history::now() - stash.time))
            }
            RowKey::StashFile(hash, file) => {
                format!(
                    "{:?}",
                    state.stash_changes.get(hash)?.iter().find(|f| f.path == *file)?.change
                )
            }
        };
        Some((term_ui::kit::ui_size::current().to_bits(), shown))
    }
}
