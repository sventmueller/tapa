//! What the Git panel lists: staged files, other changes, the stash and recent commits, each row named by a key that
//! survives a refresh.

use std::path::{Path, PathBuf};

use crate::{Commit, CommitId, EntryKind, FileState, Stash, Status, StatusEntry};
use splat_i18n::t;

use super::graph::{self, GraphRow};

/// A group of rows in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    /// Files with merge conflicts.
    Conflicts,
    /// What the next commit takes.
    Staged,
    /// Changes not staged and untracked files.
    Changes,
    /// The entries of the stash.
    Stashes,
    /// The history of HEAD and its upstream as a graph.
    Commits,
}

impl Section {
    /// Every section in the order the panel lists them.
    pub(crate) const ALL: [Section; 5] = [
        Section::Conflicts,
        Section::Staged,
        Section::Changes,
        Section::Stashes,
        Section::Commits,
    ];

    /// The section's heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Conflicts => t("Konflikte"),
            Section::Staged => t("Vorgemerkt"),
            Section::Changes => t("Änderungen"),
            Section::Stashes => t("Stashes"),
            Section::Commits => t("Graph"),
        }
    }
}

/// What happened to a file, shown as a letter and a colour and said in words to VoiceOver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Its content changed.
    Modified,
    /// It became another kind of file, such as a link.
    TypeChanged,
    /// It is new.
    Added,
    /// It is gone.
    Deleted,
    /// It was renamed.
    Renamed,
    /// It was copied.
    Copied,
    /// It has merge conflicts.
    Conflict,
    /// git does not track it.
    Untracked,
}

impl Mark {
    fn of(state: FileState) -> Option<Mark> {
        Some(match state {
            FileState::Unmodified => return None,
            FileState::Modified => Mark::Modified,
            FileState::TypeChanged => Mark::TypeChanged,
            FileState::Added => Mark::Added,
            FileState::Deleted => Mark::Deleted,
            FileState::Renamed => Mark::Renamed,
            FileState::Copied => Mark::Copied,
            FileState::Unmerged => Mark::Conflict,
        })
    }

    /// The letter the row shows, as git status prints it.
    pub fn letter(self) -> &'static str {
        match self {
            Mark::Modified => "M",
            Mark::TypeChanged => "T",
            Mark::Added => "A",
            Mark::Deleted => "D",
            Mark::Renamed => "R",
            Mark::Copied => "C",
            Mark::Conflict => "U",
            Mark::Untracked => "?",
        }
    }

    /// The mark in words, for VoiceOver.
    pub fn word(self) -> &'static str {
        match self {
            Mark::Modified => t("geändert"),
            Mark::TypeChanged => t("Typ geändert"),
            Mark::Added => t("neu"),
            Mark::Deleted => t("gelöscht"),
            Mark::Renamed => t("umbenannt"),
            Mark::Copied => t("kopiert"),
            Mark::Conflict => t("Konflikt"),
            Mark::Untracked => t("nicht versioniert"),
        }
    }
}

/// A file in one section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// The file as git status reported it.
    pub entry: StatusEntry,
    /// What happened to it in this section.
    pub mark: Mark,
}

impl FileRow {
    /// The file, relative to the repository's root.
    pub fn path(&self) -> &Path {
        &self.entry.path
    }

    /// The path it had before a rename or copy in the index.
    pub fn from(&self) -> Option<&Path> {
        match &self.entry.kind {
            EntryKind::Renamed { from, .. } | EntryKind::Copied { from, .. } => Some(from),
            _ => None,
        }
    }

    /// Whether git does not track the file.
    pub fn untracked(&self) -> bool {
        self.mark == Mark::Untracked
    }
}

/// The rows of the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    /// Files with merge conflicts, by path.
    pub conflicts: Vec<FileRow>,
    /// Files the index changed, by path.
    pub staged: Vec<FileRow>,
    /// Files the working tree changed and untracked files, by path.
    pub changes: Vec<FileRow>,
    /// The entries of the stash.
    pub stashes: Vec<Stash>,
    /// The history read, children before parents.
    pub commits: Vec<Commit>,
    /// The graph's row of each commit.
    pub graph: Vec<GraphRow>,
    /// Whether the history goes on past the commits read.
    pub more: bool,
}

impl Model {
    /// The rows of `status`, `stashes` and `commits`, read up to `limit` of them.
    pub fn new(status: &Status, commits: Vec<Commit>, stashes: Vec<Stash>, limit: usize) -> Model {
        let mut model = Model {
            graph: graph::layout(&commits),
            more: commits.len() >= limit,
            commits,
            stashes,
            ..Model::default()
        };
        for entry in &status.entries {
            match &entry.kind {
                EntryKind::Ignored => {}
                EntryKind::Untracked => model.changes.push(FileRow {
                    entry: entry.clone(),
                    mark: Mark::Untracked,
                }),
                EntryKind::Unmerged(_) => model.conflicts.push(FileRow {
                    entry: entry.clone(),
                    mark: Mark::Conflict,
                }),
                _ => {
                    if let Some(mark) = Mark::of(entry.index) {
                        model.staged.push(FileRow {
                            entry: entry.clone(),
                            mark,
                        });
                    }
                    if let Some(mark) = Mark::of(entry.worktree) {
                        model.changes.push(FileRow {
                            entry: entry.clone(),
                            mark,
                        });
                    }
                }
            }
        }
        model.conflicts.sort_by(|a, b| a.path().cmp(b.path()));
        model.staged.sort_by(|a, b| a.path().cmp(b.path()));
        model.changes.sort_by(|a, b| a.path().cmp(b.path()));
        model
    }

    /// The sections that show: conflicts, staged files and stashes only when there are some, commits only after the
    /// first.
    pub fn sections(&self) -> Vec<Section> {
        Section::ALL
            .into_iter()
            .filter(|s| match s {
                Section::Conflicts => !self.conflicts.is_empty(),
                Section::Staged => !self.staged.is_empty(),
                Section::Changes => true,
                Section::Stashes => !self.stashes.is_empty(),
                Section::Commits => !self.commits.is_empty(),
            })
            .collect()
    }

    /// The files of `section`; none for sections without files.
    pub(crate) fn files(&self, section: Section) -> &[FileRow] {
        match section {
            Section::Conflicts => &self.conflicts,
            Section::Staged => &self.staged,
            Section::Changes => &self.changes,
            Section::Stashes | Section::Commits => &[],
        }
    }

    /// How many rows `section` has directly below it.
    pub fn len(&self, section: Section) -> usize {
        match section {
            Section::Commits => self.commits.len(),
            Section::Stashes => self.stashes.len(),
            other => self.files(other).len(),
        }
    }

    /// The keys of the rows in `section`.
    pub fn keys(&self, section: Section) -> Vec<RowKey> {
        match section {
            Section::Commits => self
                .commits
                .iter()
                .map(|c| RowKey::Commit(c.hash.clone()))
                .chain(self.more.then_some(RowKey::More))
                .collect(),
            Section::Stashes => self.stashes.iter().map(|s| RowKey::Stash(s.hash.clone())).collect(),
            other => self
                .files(other)
                .iter()
                .map(|f| RowKey::File(other, f.path().to_path_buf()))
                .collect(),
        }
    }

    /// The file at `path` in `section`.
    pub fn file(&self, section: Section, path: &Path) -> Option<&FileRow> {
        self.files(section).iter().find(|f| f.path() == path)
    }

    /// The stash entry whose commit is `hash`.
    pub fn stash(&self, hash: &CommitId) -> Option<&Stash> {
        self.stashes.iter().find(|s| s.hash == *hash)
    }

    /// The commit `hash`, when it was read.
    pub fn commit(&self, hash: &CommitId) -> Option<&Commit> {
        self.commits.iter().find(|c| c.hash == *hash)
    }

    /// The commit `hash` with its row of the graph.
    pub fn commit_row(&self, hash: &CommitId) -> Option<(&Commit, &GraphRow)> {
        let index = self.commits.iter().position(|c| c.hash == *hash)?;
        Some((&self.commits[index], self.graph.get(index)?))
    }

    /// The lanes that run past the end of the history read, for the row that reads more.
    pub fn last_lanes(&self) -> GraphRow {
        self.graph.last().map(GraphRow::through).unwrap_or_default()
    }

    /// How many files differ from HEAD in the index or the working tree, for the badge.
    pub fn changed_files(&self) -> usize {
        let mut paths: Vec<&Path> = self
            .conflicts
            .iter()
            .chain(&self.staged)
            .chain(&self.changes)
            .map(FileRow::path)
            .collect();
        paths.sort();
        paths.dedup();
        paths.len()
    }
}

/// Names a row across refreshes, so the list keeps what was open and selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    /// A section's heading.
    Section(Section),
    /// A file of a section, relative to the repository's root.
    File(Section, PathBuf),
    /// A commit.
    Commit(CommitId),
    /// A file a commit changed: the commit's hash, the file relative to the repository's root.
    CommitFile(CommitId, PathBuf),
    /// The row that reads more of the history.
    More,
    /// A stash entry, by its commit.
    Stash(CommitId),
    /// A file a stash entry changed: the entry's commit, the file relative to the repository's root.
    StashFile(CommitId, PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BranchStatus, Conflict};

    fn entry(path: &str, kind: EntryKind, index: FileState, worktree: FileState) -> StatusEntry {
        StatusEntry {
            path: PathBuf::from(path),
            kind,
            index,
            worktree,
            submodule: None,
        }
    }

    fn status(entries: Vec<StatusEntry>) -> Status {
        Status {
            branch: BranchStatus::default(),
            stash: 0,
            entries,
        }
    }

    #[test]
    fn a_file_changed_in_both_places_shows_in_both_sections_and_counts_once() {
        let model = Model::new(
            &status(vec![
                entry("b.rs", EntryKind::Ordinary, FileState::Modified, FileState::Modified),
                entry("a.rs", EntryKind::Ordinary, FileState::Added, FileState::Unmodified),
                entry(
                    "new.txt",
                    EntryKind::Untracked,
                    FileState::Unmodified,
                    FileState::Unmodified,
                ),
                entry(
                    "both.rs",
                    EntryKind::Unmerged(Conflict::BothModified),
                    FileState::Unmerged,
                    FileState::Unmerged,
                ),
                entry(
                    "target",
                    EntryKind::Ignored,
                    FileState::Unmodified,
                    FileState::Unmodified,
                ),
            ]),
            vec![],
            vec![],
            50,
        );
        let staged: Vec<_> = model.staged.iter().map(|f| (f.path().to_path_buf(), f.mark)).collect();
        assert_eq!(staged, [("a.rs".into(), Mark::Added), ("b.rs".into(), Mark::Modified)]);
        let changes: Vec<_> = model.changes.iter().map(|f| f.mark.letter()).collect();
        assert_eq!(changes, ["M", "?"]);
        assert_eq!(model.conflicts.len(), 1);
        assert_eq!(model.conflicts[0].mark.letter(), "U");
        assert_eq!(model.changed_files(), 4);
        assert_eq!(
            model.sections(),
            [Section::Conflicts, Section::Staged, Section::Changes]
        );
    }
}
