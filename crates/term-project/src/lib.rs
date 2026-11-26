//! A project folder without a window: its tree read one folder at a time, changes on disk and the `.editorconfig`
//! settings of its files.
//!
//! The watcher runs a thread of its own and answers through a callback; everything else blocks and belongs on a
//! background thread.
//!
//! [`files_panel`] is what the app's files panel decides without AppKit.

#![forbid(unsafe_code)]

pub mod editorconfig;
pub mod files_panel;
pub mod order;
pub mod tree;
pub mod watch;

mod rules;

pub use tree::{Entry, Kind, Tree, TreeOptions};
pub use watch::{Changes, FileChange, RescanReason, WatchEvent, Watcher};

/// Indexes the files below `root` using the project's ignore rules. Run on a worker thread.
pub fn file_index(root: &std::path::Path, cancel: &term_base::Cancel) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in rules::walker(root, root, true).build() {
        if cancel.is_cancelled() {
            return Vec::new();
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if entry.file_type().is_some_and(|kind| {
            kind.is_file() || (kind.is_symlink() && std::fs::metadata(path).is_ok_and(|meta| meta.is_file()))
        }) && entry.file_name() != ".DS_Store"
        {
            files.push(entry.into_path());
        }
    }
    files.sort_unstable();
    files
}

#[cfg(test)]
#[test]
fn the_file_index_walks_subfolders_and_applies_ignore_rules() {
    let root = testing::scratch("file-index");
    testing::write(root.path(), ".gitignore", "build/\n");
    testing::write(root.path(), "src/main.rs", "");
    testing::write(root.path(), "build/output.txt", "");
    let cancel = term_base::Cancel::new();
    assert_eq!(
        file_index(root.path(), &cancel),
        [root.path().join(".gitignore"), root.path().join("src/main.rs")]
    );
    cancel.cancel();
    assert!(file_index(root.path(), &cancel).is_empty());
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::Path;

    pub(crate) use term_base::testing::scratch;

    /// Writes `content` to `root/relative`, creating the folders on the way.
    pub(crate) fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Lets a bound on disk work hold in unoptimised test builds, where the walk runs about twice as slow.
    pub(crate) fn walk_factor() -> u32 {
        if cfg!(debug_assertions) { 3 } else { 1 }
    }

    #[test]
    fn the_long_lived_parts_can_move_to_and_be_shared_between_background_threads() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<crate::Tree>();
        send_and_sync::<crate::Watcher>();
    }
}
