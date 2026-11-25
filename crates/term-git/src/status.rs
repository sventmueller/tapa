//! `git status --porcelain=v2 -z`: the branch, its upstream, the stash, and every changed, unmerged, untracked or
//! ignored file.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::id::CommitId;
use crate::path::from_bytes;
use crate::repo::Repository;

/// Which untracked files status lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Untracked {
    /// None.
    No,
    /// Untracked folders as one entry each, as `git status` shows them.
    Folders,
    /// Every untracked file.
    #[default]
    All,
}

/// What status lists beyond tracked changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusOptions {
    /// Which untracked files.
    pub untracked: Untracked,
    /// Whether ignored files and folders are listed too.
    pub ignored: bool,
}

/// The state of the working tree and its branch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The branch and its upstream.
    pub branch: BranchStatus,
    /// How many entries the stash holds.
    pub stash: usize,
    /// The files that differ from HEAD or the index, are unmerged, untracked or ignored.
    pub entries: Vec<StatusEntry>,
}

/// The current branch as status reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BranchStatus {
    /// The commit HEAD points at; `None` before the first commit.
    pub commit: Option<CommitId>,
    /// The branch's short name; `None` when HEAD is detached.
    pub head: Option<String>,
    /// The upstream, such as `origin/main`.
    pub upstream: Option<String>,
    /// Commits ahead of and behind the upstream; `None` without an upstream or when it is gone.
    pub ahead_behind: Option<(u32, u32)>,
}

/// One file in the status.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct StatusEntry {
    /// The path relative to the repository's root; for a rename, the new one.
    pub path: PathBuf,
    pub kind: EntryKind,
    /// The change between HEAD and the index.
    pub index: FileState,
    /// The change between the index and the working tree.
    pub worktree: FileState,
    /// The state of a submodule; `None` for other files.
    pub submodule: Option<Submodule>,
}

/// The kind of a status entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// A tracked file with changes.
    Ordinary,
    /// A tracked file renamed in the index.
    Renamed {
        /// The old path.
        from: PathBuf,
        /// The similarity in percent.
        score: u8,
    },
    /// A tracked file copied in the index.
    Copied {
        /// The source.
        from: PathBuf,
        /// The similarity in percent.
        score: u8,
    },
    /// A file with a merge conflict.
    Unmerged(Conflict),
    /// A file git does not track.
    Untracked,
    /// A file `.gitignore` excludes.
    Ignored,
}

/// One side's change to a file, the letters of `git status --short`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// `.`: unchanged.
    Unmodified,
    /// `M`: changed.
    Modified,
    /// `T`: its type changed, such as a file becoming a symlink.
    TypeChanged,
    /// `A`: added.
    Added,
    /// `D`: deleted.
    Deleted,
    /// `R`: renamed.
    Renamed,
    /// `C`: copied.
    Copied,
    /// `U`: unmerged.
    Unmerged,
}

/// How both sides of a merge left a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// `DD`.
    BothDeleted,
    /// `AU`.
    AddedByUs,
    /// `UD`.
    DeletedByThem,
    /// `UA`.
    AddedByThem,
    /// `DU`.
    DeletedByUs,
    /// `AA`.
    BothAdded,
    /// `UU`.
    BothModified,
}

/// What changed inside a submodule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submodule {
    /// It points at another commit.
    pub(crate) commit_changed: bool,
    /// Its tracked files changed.
    pub(crate) modified: bool,
    /// It has untracked files.
    pub(crate) untracked: bool,
}

impl Repository {
    /// The working tree's status. It takes no locks, so it never gets in the way of git in a terminal.
    pub fn status(&self, options: StatusOptions) -> Result<Status> {
        let untracked = match options.untracked {
            Untracked::No => "--untracked-files=no",
            Untracked::Folders => "--untracked-files=normal",
            Untracked::All => "--untracked-files=all",
        };
        let mut call = self
            .call([
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--show-stash",
            ])
            .args([untracked]);
        if options.ignored {
            call = call.args(["--ignored=matching"]);
        }
        let output = self.git().ok(call)?;
        parse(&output.stdout)
    }
}

/// The status in `output`.
pub(crate) fn parse(output: &[u8]) -> Result<Status> {
    let mut status = Status::default();
    let mut records = crate::path::nul_fields(output);
    while let Some(record) = records.next() {
        let unexpected = || Error::Unexpected {
            what: String::from_utf8_lossy(record).into_owned(),
        };
        match record.first() {
            Some(b'#') => header(&mut status, &String::from_utf8_lossy(record)),
            Some(b'1') => status.entries.push(ordinary(record).ok_or_else(unexpected)?),
            Some(b'2') => {
                let from = records.next().ok_or_else(unexpected)?;
                status.entries.push(renamed(record, from).ok_or_else(unexpected)?);
            }
            Some(b'u') => status.entries.push(unmerged(record).ok_or_else(unexpected)?),
            Some(b'?') => status
                .entries
                .push(other(record, EntryKind::Untracked).ok_or_else(unexpected)?),
            Some(b'!') => status
                .entries
                .push(other(record, EntryKind::Ignored).ok_or_else(unexpected)?),
            _ => return Err(unexpected()),
        }
    }
    Ok(status)
}

fn header(status: &mut Status, line: &str) {
    let mut words = line.trim_start_matches('#').split_whitespace();
    let (Some(key), Some(value)) = (words.next(), words.next()) else {
        return;
    };
    match key {
        "branch.oid" => status.branch.commit = (value != "(initial)").then(|| CommitId::from_git(value)),
        "branch.head" => status.branch.head = (value != "(detached)").then(|| value.to_string()),
        "branch.upstream" => status.branch.upstream = Some(value.to_string()),
        "branch.ab" => {
            let ahead = value.trim_start_matches('+').parse().ok();
            let behind = words.next().and_then(|b| b.trim_start_matches('-').parse().ok());
            status.branch.ahead_behind = ahead.zip(behind);
        }
        "stash" => status.stash = value.parse().unwrap_or(0),
        _ => {}
    }
}

/// The first `count` space-separated fields of `record` and the rest, which is a path and may hold spaces.
fn fields(record: &[u8], count: usize) -> Option<(Vec<&[u8]>, &[u8])> {
    let mut parts = Vec::with_capacity(count);
    let mut rest = record;
    for _ in 0..count {
        let space = rest.iter().position(|&b| b == b' ')?;
        parts.push(&rest[..space]);
        rest = &rest[space + 1..];
    }
    Some((parts, rest))
}

fn states(xy: &[u8]) -> Option<(FileState, FileState)> {
    match xy {
        [x, y] => Some((state(*x)?, state(*y)?)),
        _ => None,
    }
}

fn state(letter: u8) -> Option<FileState> {
    Some(match letter {
        b'.' => FileState::Unmodified,
        b'M' => FileState::Modified,
        b'T' => FileState::TypeChanged,
        b'A' => FileState::Added,
        b'D' => FileState::Deleted,
        b'R' => FileState::Renamed,
        b'C' => FileState::Copied,
        b'U' => FileState::Unmerged,
        _ => return None,
    })
}

fn submodule(field: &[u8]) -> Option<Submodule> {
    match field {
        [b'S', c, m, u] => Some(Submodule {
            commit_changed: *c == b'C',
            modified: *m == b'M',
            untracked: *u == b'U',
        }),
        _ => None,
    }
}

fn ordinary(record: &[u8]) -> Option<StatusEntry> {
    let (parts, path) = fields(record, 8)?;
    let (index, worktree) = states(parts[1])?;
    Some(StatusEntry {
        path: from_bytes(path),
        kind: EntryKind::Ordinary,
        index,
        worktree,
        submodule: submodule(parts[2]),
    })
}

fn renamed(record: &[u8], from: &[u8]) -> Option<StatusEntry> {
    let (parts, path) = fields(record, 9)?;
    let (index, worktree) = states(parts[1])?;
    let score_field = std::str::from_utf8(parts[8]).ok()?;
    let score = score_field.get(1..)?.parse().ok()?;
    let from = from_bytes(from);
    let kind = match score_field.as_bytes().first()? {
        b'R' => EntryKind::Renamed { from, score },
        b'C' => EntryKind::Copied { from, score },
        _ => return None,
    };
    Some(StatusEntry {
        path: from_bytes(path),
        kind,
        index,
        worktree,
        submodule: submodule(parts[2]),
    })
}

fn unmerged(record: &[u8]) -> Option<StatusEntry> {
    let (parts, path) = fields(record, 10)?;
    let conflict = match parts[1] {
        b"DD" => Conflict::BothDeleted,
        b"AU" => Conflict::AddedByUs,
        b"UD" => Conflict::DeletedByThem,
        b"UA" => Conflict::AddedByThem,
        b"DU" => Conflict::DeletedByUs,
        b"AA" => Conflict::BothAdded,
        b"UU" => Conflict::BothModified,
        _ => return None,
    };
    Some(StatusEntry {
        path: from_bytes(path),
        kind: EntryKind::Unmerged(conflict),
        index: FileState::Unmerged,
        worktree: FileState::Unmerged,
        submodule: submodule(parts[2]),
    })
}

fn other(record: &[u8], kind: EntryKind) -> Option<StatusEntry> {
    let path = record.get(2..)?;
    Some(StatusEntry {
        path: from_bytes(path),
        kind,
        index: FileState::Unmodified,
        worktree: FileState::Unmodified,
        submodule: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "814f4a422927b82f5f8a43f8fab6d3839e3983f2";

    #[test]
    fn headers_give_branch_upstream_distance_and_stash() {
        let output = format!(
            "# branch.oid {HASH}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -3\0# stash 4\0"
        );
        let status = parse(output.as_bytes()).unwrap();
        assert_eq!(status.branch.commit.as_ref().map(CommitId::as_str), Some(HASH));
        assert_eq!(status.branch.head.as_deref(), Some("main"));
        assert_eq!(status.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.branch.ahead_behind, Some((2, 3)));
        assert_eq!(status.stash, 4);
        assert!(status.entries.is_empty());
    }

    #[test]
    fn a_new_repository_and_a_detached_head_have_no_names() {
        let status = parse(b"# branch.oid (initial)\0# branch.head (detached)\0").unwrap();
        assert_eq!(status.branch, BranchStatus::default());
    }

    #[test]
    fn every_kind_of_entry_is_read_with_spaces_newlines_and_unicode_in_paths() {
        let output = format!(
            "1 .M N... 100644 100644 100644 {HASH} {HASH} sp ace.txt\0\
             1 A. N... 000000 100644 100644 {HASH} {HASH} new\nline.txt\0\
             2 RM N... 100644 100644 100644 {HASH} {HASH} R87 moved sp.txt\0old sp.txt\0\
             2 C. N... 100644 100644 100644 {HASH} {HASH} C100 copy.txt\0orig.txt\0\
             u UU N... 100644 100644 100644 100644 {HASH} {HASH} {HASH} conflict ü.txt\0\
             1 .M SCMU 160000 160000 160000 {HASH} {HASH} vendor/lib\0\
             ? untracked ö.txt\0\
             ! ignored/\0"
        );
        let entries = parse(output.as_bytes()).unwrap().entries;
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[0].path, PathBuf::from("sp ace.txt"));
        assert_eq!(
            (entries[0].index, entries[0].worktree),
            (FileState::Unmodified, FileState::Modified)
        );
        assert_eq!(entries[1].path, PathBuf::from("new\nline.txt"));
        assert_eq!(entries[1].index, FileState::Added);
        assert_eq!(entries[2].path, PathBuf::from("moved sp.txt"));
        assert_eq!(
            entries[2].kind,
            EntryKind::Renamed {
                from: "old sp.txt".into(),
                score: 87
            }
        );
        assert_eq!(
            entries[3].kind,
            EntryKind::Copied {
                from: "orig.txt".into(),
                score: 100
            }
        );
        assert_eq!(entries[4].kind, EntryKind::Unmerged(Conflict::BothModified));
        assert_eq!(entries[4].path, PathBuf::from("conflict ü.txt"));
        assert_eq!(
            entries[5].submodule,
            Some(Submodule {
                commit_changed: true,
                modified: true,
                untracked: true
            })
        );
        assert_eq!(entries[6].kind, EntryKind::Untracked);
        assert_eq!(entries[6].path, PathBuf::from("untracked ö.txt"));
        assert_eq!(entries[7].kind, EntryKind::Ignored);
    }

    #[test]
    fn a_record_git_never_writes_is_an_error() {
        assert!(matches!(parse(b"x what\0"), Err(Error::Unexpected { .. })));
        assert!(matches!(parse(b"1 .M\0"), Err(Error::Unexpected { .. })));
    }
}
