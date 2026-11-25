//! The files a commit or a stash entry changed, as `git diff --name-status -z` lists them.

use std::path::PathBuf;

use crate::diff::Change;
use crate::path::from_bytes;

/// A file a commit or a stash entry changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Its path now, relative to the root; for a deleted file the one it had.
    pub path: PathBuf,
    /// What happened to it; an untracked file counts as added.
    pub change: Change,
    /// The path it had before a rename or copy.
    pub from: Option<PathBuf>,
}

/// The files of `git diff --name-status -z`: a status letter with a score, then one path, or two for a rename or
/// copy.
pub(crate) fn name_status(output: &[u8]) -> Vec<ChangedFile> {
    let mut fields = output.split(|&b| b == 0).filter(|f| !f.is_empty());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let score = std::str::from_utf8(&status[1..])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let change = match status.first() {
            Some(b'A') => Change::Added,
            Some(b'D') => Change::Deleted,
            Some(b'R') => Change::Renamed { similarity: score },
            Some(b'C') => Change::Copied { similarity: score },
            Some(b'U') => Change::Unmerged,
            _ => Change::Modified,
        };
        let from = matches!(change, Change::Renamed { .. } | Change::Copied { .. })
            .then(|| fields.next().map(from_bytes))
            .flatten();
        if let Some(path) = fields.next() {
            files.push(ChangedFile {
                path: from_bytes(path),
                change,
                from,
            });
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_reads_letters_scores_and_the_new_path_of_a_rename() {
        let files = name_status(b"M\0src/a.rs\0R087\0old.rs\0new.rs\0A\0n\xc3\xa4u.txt\0D\0gone\0");
        let read: Vec<_> = files.iter().map(|f| (f.path.display().to_string(), f.change)).collect();
        assert_eq!(files[1].from, Some(PathBuf::from("old.rs")));
        assert_eq!(files[0].from, None);
        assert_eq!(
            read,
            [
                ("src/a.rs".to_string(), Change::Modified),
                ("new.rs".to_string(), Change::Renamed { similarity: 87 }),
                ("näu.txt".to_string(), Change::Added),
                ("gone".to_string(), Change::Deleted),
            ]
        );
    }
}
