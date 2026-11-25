//! The Git panel's model: what it shows for its folder, what runs, which rows are open, and how a refresh and
//! the opening of a row change that. It knows no AppKit; the panel's controller reads git and draws.

pub(crate) use term_git::panel::{FileRow, Mark, Model, RowKey, Section};

use std::collections::{HashMap, HashSet};

use term_git::{Branch, BranchStatus, Cancel, Commit, CommitId, Operation, Repository, Stash, StashApply, Status};

use super::text::Problem;

/// How many commits the graph shows at first, and how many more each time the user asks.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub const HISTORY_PAGE: usize = 50;

/// What the panel can show for its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Phase {
    #[default]
    Loading,
    NoGit,
    NotRepository,
    Ready,
    Failed(String),
}

/// Something running that keeps other operations from starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Task {
    Commit,
    Fetch,
    Pull,
    Push,
    Switch,
    Continue,
    Abort,
    /// Changes go into the stash, or an entry comes back or goes.
    Stash,
    /// A new worktree is made for a branch.
    Worktree,
}

pub(crate) struct Busy {
    pub(crate) task: Task,
    pub(crate) cancel: Option<Cancel>,
}

#[derive(Default)]
struct Refresh {
    scheduled: bool,
    running: bool,
    again: bool,
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) phase: Phase,
    pub(crate) repo: Option<Repository>,
    pub(crate) branch: BranchStatus,
    pub(crate) branches: Vec<Branch>,
    /// The merge or rebase that stopped at conflicts.
    pub(crate) operation: Option<Operation>,
    /// The stash apply that stopped at conflicts.
    pub(crate) stash_apply: Option<StashApply>,
    /// The stash entries whose files show.
    pub(crate) open_stashes: HashSet<CommitId>,
    /// What stash entries changed, by commit, as far as read.
    pub(crate) stash_changes: HashMap<CommitId, Vec<term_git::ChangedFile>>,
    /// The stash entries whose changes are being read.
    pub(crate) reading_stashes: HashSet<CommitId>,
    pub(crate) model: Model,
    pub(crate) collapsed: HashSet<Section>,
    /// The commits whose files show.
    pub(crate) open_commits: HashSet<CommitId>,
    /// What commits changed, by hash, as far as read.
    pub(crate) commit_changes: HashMap<CommitId, term_git::CommitChanges>,
    /// The commits whose changes are being read.
    pub(crate) reading_changes: HashSet<CommitId>,
    /// How many more pages of history the user asked for.
    pub(crate) more_pages: usize,
    /// What follows the commit that runs: a push, or a pull and then a push.
    pub(crate) after_commit: Option<Task>,
    /// Whether a push follows the pull that runs.
    pub(crate) push_after_pull: bool,
    pub(crate) busy: Option<Busy>,
    pub(crate) problem: Option<Problem>,
    /// The rows the open context menu acts on.
    pub(crate) menu_keys: Vec<RowKey>,
    /// The only rows the last refresh changed, when no other row did; the list then reloads just these.
    pub(crate) reload_only: Option<Vec<RowKey>>,
    refresh: Refresh,
}

/// What a refresh read from git.
pub(crate) enum Loaded {
    NoGit,
    NotRepository,
    Failed(Problem),
    Ready {
        repo: Repository,
        status: Box<Status>,
        branches: Vec<Branch>,
        log: Vec<Commit>,
        operation: Option<Operation>,
        stash: Box<StashRead>,
    },
}

/// The stash as a refresh read it: its entries and the apply that waits at its conflicts.
pub(crate) struct StashRead {
    pub(crate) entries: Vec<Stash>,
    pub(crate) apply: Option<StashApply>,
}

/// What the controller does when a refresh is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start {
    /// A read runs; another follows it.
    Queued,
    /// Read git with up to `limit` commits.
    Read { limit: usize },
}

/// What the controller reads when a row opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Read {
    /// The files a commit changed, by hash.
    CommitChanges(CommitId),
    /// The files a stash entry changed, by its commit.
    StashChanges(CommitId),
}

impl State {
    /// How many commits the graph reads.
    pub(crate) fn history_limit(&self) -> usize {
        HISTORY_PAGE * (self.more_pages + 1)
    }

    /// Notes that changes on disk ask for a refresh; whether the controller has to schedule one, which it does not
    /// while one is scheduled already.
    pub(crate) fn schedule_refresh(&mut self) -> bool {
        !std::mem::replace(&mut self.refresh.scheduled, true)
    }

    /// Starts a refresh now, or right after the one that runs.
    pub(crate) fn start_refresh(&mut self) -> Start {
        self.refresh.scheduled = false;
        if self.refresh.running {
            self.refresh.again = true;
            Start::Queued
        } else {
            self.refresh.running = true;
            Start::Read {
                limit: self.history_limit(),
            }
        }
    }

    /// The read could not start; the controller schedules it again.
    pub(crate) fn refresh_not_started(&mut self) {
        self.refresh.running = false;
    }

    /// The read is over; whether another one was asked for while it ran.
    pub(crate) fn refresh_finished(&mut self) -> bool {
        self.refresh.running = false;
        std::mem::take(&mut self.refresh.again)
    }

    /// Takes what a refresh read; whether anything the panel shows changed.
    pub(crate) fn apply(&mut self, loaded: Loaded) -> bool {
        let was_ready = self.phase == Phase::Ready;
        self.reload_only = None;
        match loaded {
            Loaded::NoGit => (self.phase, self.repo) = (Phase::NoGit, None),
            Loaded::NotRepository => (self.phase, self.repo) = (Phase::NotRepository, None),
            Loaded::Failed(problem) if was_ready => self.problem = Some(problem),
            Loaded::Failed(problem) => (self.phase, self.repo) = (Phase::Failed(problem.message), None),
            Loaded::Ready {
                repo,
                status,
                branches,
                log,
                operation,
                stash,
            } => {
                let StashRead {
                    entries: stashes,
                    apply,
                } = *stash;
                let model = Model::new(&status, log, stashes, self.history_limit());
                if was_ready && self.shows_already(&model, &status.branch, &branches, &operation, &apply) {
                    return false;
                }
                let heads_alike = self.branch == status.branch
                    && self.branches == branches
                    && self.operation == operation
                    && self.stash_apply == apply;
                if was_ready && heads_alike {
                    self.reload_only = changed_files(&self.model, &model);
                }
                self.phase = Phase::Ready;
                self.operation = operation;
                self.stash_apply = apply;
                self.repo = Some(repo);
                self.branch = status.branch.clone();
                self.branches = branches;
                self.model = model;
                let kept: HashSet<&CommitId> = self.model.stashes.iter().map(|s| &s.hash).collect();
                self.open_stashes.retain(|hash| kept.contains(hash));
            }
        }
        if self.phase != Phase::Ready {
            self.model = Model::default();
        }
        self.forget_gone_changes();
        if self.model.conflicts.is_empty() && self.problem.as_ref().is_some_and(|p| p.conflicts) {
            self.problem = None;
        }
        true
    }

    /// Forgets what the commits and stash entries changed that the history and the stash no longer hold.
    fn forget_gone_changes(&mut self) {
        let commits: HashSet<&CommitId> = self.model.commits.iter().map(|c| &c.hash).collect();
        self.commit_changes.retain(|hash, _| commits.contains(hash));
        let stashes: HashSet<&CommitId> = self.model.stashes.iter().map(|s| &s.hash).collect();
        self.stash_changes.retain(|hash, _| stashes.contains(hash));
    }

    /// Takes what the commit `hash` changed, unless the history no longer holds it; whether it did.
    pub(crate) fn note_commit_changes(&mut self, hash: &CommitId, changes: term_git::CommitChanges) -> bool {
        let held = self.model.commit(hash).is_some();
        if held {
            self.commit_changes.insert(hash.clone(), changes);
        }
        held
    }

    /// Takes what the stash entry `hash` changed, unless the stash no longer holds it; whether it did.
    pub(crate) fn note_stash_changes(&mut self, hash: &CommitId, files: Vec<term_git::ChangedFile>) -> bool {
        let held = self.model.stash(hash).is_some();
        if held {
            self.stash_changes.insert(hash.clone(), files);
        }
        held
    }

    /// Whether a read that yields these rows, branch, branches and stopped operations is what the panel shows already.
    fn shows_already(
        &self,
        model: &Model,
        branch: &BranchStatus,
        branches: &[Branch],
        operation: &Option<Operation>,
        stash_apply: &Option<StashApply>,
    ) -> bool {
        self.model == *model
            && self.branch == *branch
            && self.branches == branches
            && self.operation == *operation
            && self.stash_apply == *stash_apply
    }

    /// The rows below `parent`, the sections for the top.
    pub(crate) fn children(&self, parent: Option<&RowKey>) -> Vec<RowKey> {
        match parent {
            None => self.model.sections().into_iter().map(RowKey::Section).collect(),
            Some(RowKey::Section(section)) => self.model.keys(*section),
            Some(RowKey::Stash(hash)) => self.stash_changes.get(hash).map_or_else(Vec::new, |files| {
                files
                    .iter()
                    .map(|f| RowKey::StashFile(hash.clone(), f.path.clone()))
                    .collect()
            }),
            Some(RowKey::Commit(hash)) => self.commit_changes.get(hash).map_or_else(Vec::new, |changes| {
                changes
                    .files
                    .iter()
                    .map(|f| RowKey::CommitFile(hash.clone(), f.path.clone()))
                    .collect()
            }),
            _ => vec![],
        }
    }

    /// The keys of every row the panel has, open or not.
    pub(crate) fn all_keys(&self) -> std::collections::HashSet<RowKey> {
        let mut all = std::collections::HashSet::new();
        let mut pending = self.children(None);
        while let Some(key) = pending.pop() {
            pending.extend(self.children(Some(&key)));
            all.insert(key);
        }
        all
    }

    /// Whether the row of `key` can open.
    pub(crate) fn expandable(&self, key: &RowKey) -> bool {
        matches!(key, RowKey::Section(_) | RowKey::Stash(_) | RowKey::Commit(_))
    }

    /// Notes that the row of `key` opened or closed; what the controller has to read for it.
    pub(crate) fn expansion(&mut self, key: RowKey, expanded: bool) -> Option<Read> {
        match key {
            RowKey::Section(section) => {
                if expanded {
                    self.collapsed.remove(&section);
                } else {
                    self.collapsed.insert(section);
                }
            }
            RowKey::Commit(hash) => {
                if expanded {
                    self.open_commits.insert(hash.clone());
                    return Some(Read::CommitChanges(hash));
                }
                self.open_commits.remove(&hash);
            }
            RowKey::Stash(hash) => {
                if expanded {
                    self.open_stashes.insert(hash.clone());
                    return Some(Read::StashChanges(hash));
                }
                self.open_stashes.remove(&hash);
            }
            _ => {}
        }
        None
    }
}

/// The file rows that changed from `old` to `new` when nothing else did and every section kept its files in their
/// order; `None` when more changed.
fn changed_files(old: &Model, new: &Model) -> Option<Vec<RowKey>> {
    let rest_alike =
        old.stashes == new.stashes && old.commits == new.commits && old.graph == new.graph && old.more == new.more;
    if !rest_alike {
        return None;
    }
    let mut keys = Vec::new();
    for (section, before, after) in [
        (Section::Conflicts, &old.conflicts, &new.conflicts),
        (Section::Staged, &old.staged, &new.staged),
        (Section::Changes, &old.changes, &new.changes),
    ] {
        if before.len() != after.len() || before.iter().zip(after).any(|(a, b)| a.path() != b.path()) {
            return None;
        }
        keys.extend(
            before
                .iter()
                .zip(after)
                .filter(|(a, b)| a != b)
                .map(|(_, b)| RowKey::File(section, b.path().to_path_buf())),
        );
    }
    Some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A commit whose hash is `digit` forty times.
    fn id(digit: char) -> CommitId {
        CommitId::parse(&digit.to_string().repeat(40)).unwrap()
    }

    #[test]
    fn a_refresh_is_scheduled_once_until_it_starts() {
        let mut state = State::default();
        assert!(state.schedule_refresh());
        assert!(!state.schedule_refresh());
        assert_eq!(state.start_refresh(), Start::Read { limit: HISTORY_PAGE });
        assert!(state.schedule_refresh());
    }

    #[test]
    fn a_refresh_asked_for_while_one_runs_follows_it() {
        let mut state = State {
            more_pages: 2,
            ..State::default()
        };
        assert_eq!(
            state.start_refresh(),
            Start::Read {
                limit: 3 * HISTORY_PAGE
            }
        );
        assert_eq!(state.start_refresh(), Start::Queued);
        assert!(state.refresh_finished());
        assert!(!state.refresh_finished());
    }

    #[test]
    fn a_failure_after_a_good_read_keeps_the_list_and_shows_the_problem() {
        let mut state = State::default();
        assert!(state.apply(Loaded::Failed(Problem::plain("broken"))));
        assert_eq!(state.phase, Phase::Failed("broken".into()));
        state.phase = Phase::Ready;
        assert!(state.apply(Loaded::Failed(Problem::plain("again"))));
        assert_eq!(state.phase, Phase::Ready);
        assert_eq!(state.problem.as_ref().map(|p| p.message.as_str()), Some("again"));
        assert!(state.apply(Loaded::NoGit));
        assert_eq!(state.phase, Phase::NoGit);
        assert_eq!(state.model, Model::default());
    }

    #[test]
    fn opening_a_commit_or_stash_reads_its_files_and_closing_forgets_it() {
        let mut state = State::default();
        let commit = RowKey::Commit(id('a'));
        assert_eq!(
            state.expansion(commit.clone(), true),
            Some(Read::CommitChanges(id('a')))
        );
        assert!(state.open_commits.contains(&id('a')));
        assert_eq!(state.expansion(commit, false), None);
        assert!(state.open_commits.is_empty());
        assert_eq!(
            state.expansion(RowKey::Stash(id('5')), true),
            Some(Read::StashChanges(id('5')))
        );
        assert_eq!(state.expansion(RowKey::Section(Section::Staged), false), None);
        assert!(state.collapsed.contains(&Section::Staged));
        assert_eq!(state.expansion(RowKey::Section(Section::Staged), true), None);
        assert!(state.collapsed.is_empty());
    }

    fn commit(digit: char) -> Commit {
        Commit {
            hash: id(digit),
            parents: vec![],
            subject: String::new(),
            body: String::new(),
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            refs: vec![],
        }
    }

    fn stash(digit: char) -> Stash {
        Stash {
            index: 0,
            hash: id(digit),
            message: String::new(),
            time: 0,
        }
    }

    #[test]
    fn what_commits_and_stash_entries_changed_is_kept_only_while_the_panel_holds_them() {
        let mut state = State::default();
        state.model.commits = vec![commit('a'), commit('b')];
        state.model.stashes = vec![stash('5')];
        assert!(state.note_commit_changes(&id('a'), term_git::CommitChanges::default()));
        assert!(state.note_commit_changes(&id('b'), term_git::CommitChanges::default()));
        assert!(!state.note_commit_changes(&id('c'), term_git::CommitChanges::default()));
        assert!(state.note_stash_changes(&id('5'), vec![]));
        assert!(!state.note_stash_changes(&id('6'), vec![]));
        state.model.commits = vec![commit('b')];
        state.forget_gone_changes();
        assert_eq!(state.commit_changes.keys().collect::<Vec<_>>(), [&id('b')]);
        assert_eq!(state.stash_changes.keys().collect::<Vec<_>>(), [&id('5')]);
        assert!(state.apply(Loaded::NoGit));
        assert!(state.commit_changes.is_empty());
        assert!(state.stash_changes.is_empty());
    }

    #[test]
    fn a_commit_shows_its_files_once_they_are_read() {
        let state = State::default();
        let commit = RowKey::Commit(id('a'));
        assert!(state.expandable(&commit));
        assert!(state.children(Some(&commit)).is_empty());
        assert!(!state.expandable(&RowKey::More));
        assert_eq!(state.children(None), vec![RowKey::Section(Section::Changes)]);
    }
}

#[cfg(test)]
mod reload_tests {
    use term_git::{EntryKind, FileState, StatusEntry};

    use super::*;

    fn row(path: &str, mark: Mark) -> FileRow {
        FileRow {
            entry: StatusEntry {
                path: path.into(),
                kind: EntryKind::Ordinary,
                index: FileState::Unmodified,
                worktree: FileState::Modified,
                submodule: None,
            },
            mark,
        }
    }

    #[test]
    fn a_refresh_that_only_changes_files_reloads_just_their_rows() {
        let old = Model {
            changes: vec![row("a.rs", Mark::Modified), row("b.rs", Mark::Modified)],
            ..Model::default()
        };
        let mut new = old.clone();
        new.changes[1].mark = Mark::Deleted;
        assert_eq!(
            changed_files(&old, &new),
            Some(vec![RowKey::File(Section::Changes, "b.rs".into())])
        );
        new.changes.push(row("c.rs", Mark::Added));
        assert_eq!(changed_files(&old, &new), None, "a new file changes the list's shape");
        let mut more = old.clone();
        more.more = true;
        assert_eq!(
            changed_files(&old, &more),
            None,
            "a change past the files reloads everything"
        );
    }
}
