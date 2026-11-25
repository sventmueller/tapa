//! The self-test's hooks of `export.rs`.

use std::path::PathBuf;

/// Where Save writes `file_name` in a self-test: `exported` of the self-test's home, since nobody is there to answer a
/// save panel.
pub(super) fn save_path(file_name: &str) -> PathBuf {
    let folder = term_base::home().join("exported");
    let _ = std::fs::create_dir_all(&folder);
    folder.join(file_name)
}
