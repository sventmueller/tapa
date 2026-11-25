//! The folders workspaces were last opened in (E22), most recent first, for the menu of the toolbar's New Workspace
//! button; kept beside the restored state in the data folder, as paths are no secret worth sealing.

use std::path::{Path, PathBuf};

/// How many folders the menu offers.
const MOST: usize = 10;
const FILE: &str = "recent-folders.json";

/// The folders, most recent first, those that no longer exist left out.
pub(crate) fn folders() -> Vec<PathBuf> {
    read(&crate::restore::folder().join(FILE))
        .into_iter()
        .filter(|folder| folder.is_dir())
        .collect()
}

/// Puts `folder` first in the list.
pub(crate) fn used(folder: &Path) {
    let dir = crate::restore::folder();
    let path = dir.join(FILE);
    let list = with(read(&path), folder);
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec(&list)
        && let Err(error) = term_base::atomic_write::write(&path, &json)
    {
        term_base::diag::report("storing the recent folders", &error);
    }
}

fn read(path: &Path) -> Vec<PathBuf> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// `list` with `folder` first and at most [`MOST`] entries.
fn with(mut list: Vec<PathBuf>, folder: &Path) -> Vec<PathBuf> {
    list.retain(|known| known != folder);
    list.insert(0, folder.to_path_buf());
    list.truncate(MOST);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_used_again_moves_to_the_front_and_the_list_stays_short() {
        let list = with(vec!["/a".into(), "/b".into()], Path::new("/b"));
        assert_eq!(list, [PathBuf::from("/b"), PathBuf::from("/a")]);
        let mut long = Vec::new();
        for n in 0..20 {
            long = with(long, Path::new(&format!("/{n}")));
        }
        assert_eq!(long.len(), MOST);
        assert_eq!(long[0], PathBuf::from("/19"));
    }
}
