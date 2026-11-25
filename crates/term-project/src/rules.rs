//! Which entries the ignore files hide, read the same way by the tree, the index and the search.

use std::path::Path;

use ignore::{IncrementalIgnore, WalkBuilder};

/// Git's own folder, never listed, indexed or searched.
pub(crate) const GIT_DIR: &str = ".git";

/// A walk from `start` inside the project at `root`.
///
/// With `honour_ignores` it follows `.gitignore`, `.ignore`, `.git/info/exclude` and the global excludes, inside a
/// repository or not; dotfiles are always visible and links are listed, never entered.
pub(crate) fn walker(root: &Path, start: &Path, honour_ignores: bool) -> WalkBuilder {
    let mut builder = WalkBuilder::new(start);
    builder
        .standard_filters(honour_ignores)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        // The global excludes are matched relative to this folder, as git matches them relative to the work tree.
        .current_dir(root)
        .filter_entry(|entry| entry.file_name() != GIT_DIR);
    builder
}

/// The ignore rules of the project at `root` as they are on disk now, for checking single paths relative to `root`.
pub(crate) fn matcher(root: &Path) -> IncrementalIgnore {
    let mut matchers = walker(root, root, true).build_matchers();
    matchers.swap_remove(0)
}

/// Whether `relative` lies in git's folder of the project or of a repository nested in it.
pub(crate) fn in_git_dir(relative: &Path) -> bool {
    relative.components().any(|c| c.as_os_str() == GIT_DIR)
}
