//! What the Git panel does: staging, discarding, committing, branches, and the remote operations with progress and
//! cancel.

use std::path::PathBuf;
use term_ui::workspace_host::Files;

use objc2::Message;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSMenuItem};
use objc2_foundation::NSFileManager;
use splat_i18n::{t, tf};
use term_git::{CommitOptions, Error, PullMode, PullOptions, Repository};

use super::model::{Busy, Phase, Task};
use super::model::{FileRow, Mark, RowKey, Section};
use super::panel::{GitPanel, with_panel};
use super::progress::{NEW_BRANCH_TAG, NEW_WORKTREE_TAG};
use super::text::Problem;
use super::{sheets, work};
use term_ui::kit::ns;

impl GitPanel {
    /// The rows of `keys` in `section`.
    fn rows(&self, keys: &[RowKey], section: Section) -> Vec<FileRow> {
        let state = self.read();
        keys.iter()
            .filter_map(|key| match key {
                RowKey::File(s, path) if *s == section => state.model.file(*s, path).cloned(),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn stage(&self, keys: Vec<RowKey>) {
        let paths: Vec<PathBuf> = self
            .rows(&keys, Section::Changes)
            .iter()
            .map(|r| r.path().to_path_buf())
            .collect();
        if !paths.is_empty() {
            self.run_local(move |repo| repo.stage(&paths));
        }
    }

    pub(crate) fn unstage(&self, keys: Vec<RowKey>) {
        let mut paths = vec![];
        for row in self.rows(&keys, Section::Staged) {
            paths.extend(row.from().map(std::path::Path::to_path_buf));
            paths.push(row.path().to_path_buf());
        }
        if !paths.is_empty() {
            self.run_local(move |repo| repo.unstage(&paths));
        }
    }

    /// Stages files whose conflicts the user resolved, which tells git they are done.
    pub(crate) fn resolve(&self, keys: Vec<RowKey>) {
        let paths: Vec<PathBuf> = self
            .rows(&keys, Section::Conflicts)
            .iter()
            .map(|r| r.path().to_path_buf())
            .collect();
        if !paths.is_empty() {
            self.run_local(move |repo| repo.stage(&paths));
        }
    }

    pub(crate) fn stage_all(&self) {
        self.run_local(Repository::stage_all);
    }

    pub(crate) fn unstage_all(&self) {
        self.run_local(Repository::unstage_all);
    }

    /// Asks, then drops unstaged changes and moves untracked files to the Trash.
    pub(crate) fn discard(&self, keys: Vec<RowKey>) {
        let rows: Vec<FileRow> = self
            .rows(&keys, Section::Changes)
            .into_iter()
            .filter(|r| r.mark != Mark::Conflict)
            .collect();
        let (Some(first), Some(window)) = (rows.first(), self.controller().and_then(|c| c.window())) else {
            return;
        };
        let name = first.path().display().to_string();
        let untracked = rows.iter().filter(|r| r.untracked()).count();
        let (message, info, action) = match (rows.len(), untracked) {
            (1, 1) => (
                tf("„{}“ in den Papierkorb legen?", &[&name]),
                t("Die Datei ist nicht versioniert; Git kann sie nicht wiederherstellen."),
                t("In den Papierkorb legen"),
            ),
            (1, _) => (
                tf("Änderungen an „{}“ verwerfen?", &[&name]),
                t("Die nicht vorgemerkten Änderungen gehen verloren. Das lässt sich nicht widerrufen."),
                t("Verwerfen"),
            ),
            (n, _) => (
                tf("Änderungen an {} Dateien verwerfen?", &[&n.to_string()]),
                t("Nicht vorgemerkte Änderungen gehen verloren, nicht versionierte Dateien kommen in den Papierkorb."),
                t("Verwerfen"),
            ),
        };
        let id = self.id();
        sheets::confirm(&window, &message, info, action, move || {
            with_panel(id, |panel| panel.discard_confirmed(rows));
        });
    }

    fn discard_confirmed(&self, rows: Vec<FileRow>) {
        let Some(root) = self.repo().map(|r| r.root().to_path_buf()) else {
            return;
        };
        let (untracked, tracked): (Vec<FileRow>, Vec<FileRow>) = rows.into_iter().partition(FileRow::untracked);
        let manager = NSFileManager::defaultManager();
        for row in &untracked {
            let url = objc2_foundation::NSURL::fileURLWithPath(&ns(&root.join(row.path()).to_string_lossy()));
            if let Err(error) = manager.trashItemAtURL_resultingItemURL_error(&url, None) {
                self.set_problem(Some(Problem::plain(tf(
                    "„{}“ ließ sich nicht in den Papierkorb legen: {}",
                    &[
                        &row.path().display().to_string(),
                        &error.localizedDescription().to_string(),
                    ],
                ))));
            }
        }
        let paths: Vec<PathBuf> = tracked.iter().map(|r| r.path().to_path_buf()).collect();
        if paths.is_empty() {
            self.start_refresh();
        } else {
            self.run_local(move |repo| repo.discard(&paths));
        }
    }

    pub(crate) fn open(&self, keys: Vec<RowKey>) {
        self.open_commit_files(&keys);
        let (Some(controller), Some(repo)) = (self.controller(), self.repo()) else {
            return;
        };
        for key in keys {
            if let RowKey::File(_, path) = key {
                let full = repo.root().join(path);
                if full.is_file() {
                    controller.open_file(&full);
                }
            }
        }
    }

    pub(crate) fn show_diff(&self, key: &RowKey, preview: bool) {
        if let RowKey::CommitFile(hash, file) = key {
            return self.show_commit_diff(hash, file, preview);
        }
        if let RowKey::StashFile(hash, file) = key {
            return self.show_stash_diff(hash, file, preview);
        }
        let (Some(controller), Some(repo), RowKey::File(section, path)) = (self.controller(), self.repo(), key) else {
            return;
        };
        super::diff::show(&controller, repo.root(), path, *section == Section::Staged, preview);
    }

    /// Runs `operation` on the git lane, then reads the repository again.
    pub(crate) fn run_local(&self, operation: impl FnOnce(&Repository) -> term_git::Result<()> + Send + 'static) {
        let Some(repo) = self.repo() else { return };
        let id = self.id();
        let queued = work::local(
            move || operation(&repo),
            move |result| with_panel(id, |panel| panel.local_done(result)),
        );
        if !queued {
            self.set_problem(Some(Problem::plain(t(
                "Git ist gerade ausgelastet. Versuch es gleich noch einmal.",
            ))));
        }
    }

    pub(super) fn local_done(&self, result: Option<term_git::Result<()>>) {
        match result {
            Some(Ok(())) => {}
            Some(Err(error)) => self.report(&error),
            None => self.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
        }
        self.start_refresh();
    }

    /// Marks `task` as running, or refuses when something else runs.
    pub(super) fn begin(&self, task: Task, cancel: Option<term_git::Cancel>) -> bool {
        {
            let mut state = self.state();
            if state.busy.is_some() || state.phase != Phase::Ready {
                return false;
            }
            state.busy = Some(Busy { task, cancel });
            state.problem = None;
        }
        self.update_busy();
        self.update_problem_row();
        true
    }

    pub(super) fn end(&self) {
        self.state().busy = None;
        self.update_busy();
    }

    /// Commits what is staged.
    pub(crate) fn commit(&self) {
        self.commit_then(None);
    }

    /// Commits, then pushes for `Task::Push`, or pulls and pushes for `Task::Pull`.
    pub(crate) fn commit_then(&self, after: Option<Task>) {
        let message = self.ui().message.text();
        let amend = self.ui().amend.state() == NSControlStateValueOn;
        let staged = !self.read().model.staged.is_empty();
        if message.trim().is_empty() && !amend {
            self.set_problem(Some(Problem::plain(t("Die Commit-Nachricht fehlt."))));
            return;
        }
        if !staged && !amend {
            self.set_problem(Some(Problem::plain(t(
                "Nichts vorgemerkt. Merke zuerst Änderungen vor (+).",
            ))));
            return;
        }
        self.state().after_commit = after;
        self.commit_now(message, amend);
    }

    fn commit_now(&self, message: String, amend: bool) {
        let Some(repo) = self.repo() else { return };
        if !self.begin(Task::Commit, None) {
            self.state().after_commit = None;
            return;
        }
        let id = self.id();
        work::long(
            move || repo.commit(&message, CommitOptions { amend, signoff: false }),
            move |result| with_panel(id, |panel| panel.committed(result)),
        );
    }

    fn committed(&self, result: Option<term_git::Result<term_git::CommitId>>) {
        self.end();
        let after = self.state().after_commit.take();
        match result {
            Some(Ok(_)) => {
                self.ui().message.setString(&ns(""));
                self.ui().amend.setState(NSControlStateValueOff);
                self.update_placeholder();
                if let Some(task) = after {
                    self.state().push_after_pull = task == Task::Pull;
                    self.remote(task);
                    return;
                }
            }
            Some(Err(error)) => self.report(&error),
            None => self.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
        }
        self.start_refresh();
    }

    /// Fetches, pulls or pushes.
    pub(crate) fn remote(&self, task: Task) {
        self.start_remote(task, PullMode::default());
    }

    /// Runs `task`, which talks to the remote; a pull joins diverged branches as `mode` says.
    fn start_remote(&self, task: Task, mode: PullMode) {
        let Some(repo) = self.repo() else { return };
        let cancel = term_git::Cancel::new();
        if !self.begin(task, Some(cancel.clone())) {
            return;
        }
        let id = self.id();
        let has_upstream = self.read().branch.upstream.is_some();
        work::long(
            move || {
                let mut progress = super::progress::sink(id);
                match task {
                    Task::Fetch => repo.fetch(&term_git::FetchOptions::default(), &mut progress, &cancel),
                    Task::Pull => {
                        let options = PullOptions {
                            mode,
                            ..PullOptions::default()
                        };
                        repo.pull(&options, &mut progress, &cancel)
                    }
                    _ => push(&repo, has_upstream, &mut progress, &cancel),
                }
            },
            move |result| with_panel(id, |panel| panel.pull_done(task, result)),
        );
    }

    /// A pull that cannot fast-forward asks how to join the branches instead of only saying so.
    fn pull_done(&self, task: Task, result: Option<term_git::Result<()>>) {
        let diverged = matches!(result, Some(Err(Error::NotFastForward { .. })));
        let window = self.controller().and_then(|c| c.window());
        let (true, Task::Pull, Some(window)) = (diverged, task, window) else {
            self.remote_done(result);
            return;
        };
        self.end();
        let id = self.id();
        sheets::pull_mode(&window, move |mode| {
            with_panel(id, |panel| match mode {
                Some(mode) => panel.start_remote(Task::Pull, mode),
                None => {
                    panel.state().push_after_pull = false;
                    panel.start_refresh();
                }
            });
        });
    }

    fn remote_done(&self, result: Option<term_git::Result<()>>) {
        self.end();
        let push = std::mem::take(&mut self.state().push_after_pull);
        if push && matches!(result, Some(Ok(()))) {
            self.remote(Task::Push);
            return;
        }
        match result {
            Some(Ok(())) | Some(Err(Error::Cancelled)) => {}
            Some(Err(error)) => self.report(&error),
            None => self.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
        }
        self.start_refresh();
    }

    /// The branch popup: switches to the chosen branch, or asks for a new one.
    pub(crate) fn branch_chosen(&self, sender: Option<&AnyObject>) {
        let item = sender
            .and_then(|s| s.downcast_ref::<objc2_app_kit::NSPopUpButton>())
            .and_then(|popup| popup.selectedItem())
            .or_else(|| sender.and_then(|s| s.downcast_ref::<NSMenuItem>()).map(|i| i.retain()));
        let Some(item) = item else { return };
        if item.tag() == NEW_BRANCH_TAG {
            self.update_header();
            self.new_branch();
            return;
        }
        if item.tag() == NEW_WORKTREE_TAG {
            self.update_header();
            self.new_worktree();
            return;
        }
        let name = item
            .representedObject()
            .and_then(|o| o.downcast::<objc2_foundation::NSString>().ok())
            .map(|s| s.to_string());
        let current = self.read().branch.head.clone();
        match name {
            Some(name) if Some(&name) != current.as_ref() => self.switch_to(name),
            _ => self.update_header(),
        }
    }

    fn switch_to(&self, name: String) {
        let Some(repo) = self.repo() else { return };
        if !self.begin(Task::Switch, None) {
            self.update_header();
            return;
        }
        let id = self.id();
        work::long(
            move || repo.switch_branch(&name),
            move |result| with_panel(id, |panel| panel.remote_done(result)),
        );
    }

    pub(crate) fn new_branch(&self) {
        let Some(window) = self.controller().and_then(|c| c.window()) else {
            return;
        };
        let id = self.id();
        let info = t("Er beginnt beim aktuellen Commit, und Git wechselt gleich zu ihm.");
        sheets::branch_name(&window, t("Neuer Branch"), info, move |name| {
            with_panel(id, |panel| {
                let Some(repo) = panel.repo() else { return };
                if !panel.begin(Task::Switch, None) {
                    return;
                }
                work::long(
                    move || repo.switch_to_new_branch(&name, None),
                    move |result| with_panel(id, |panel| panel.remote_done(result)),
                );
            });
        });
    }

    /// Asks for a branch, makes a worktree for it beside the repository and opens that as a workspace of its own.
    pub(crate) fn new_worktree(&self) {
        let (Some(window), Some(repo)) = (self.controller().and_then(|c| c.window()), self.repo()) else {
            return;
        };
        let id = self.id();
        let info =
            t("Er bekommt einen neuen Branch ab dem aktuellen Commit und öffnet sich als eigener Arbeitsbereich.");
        sheets::branch_name(&window, t("Neuer Worktree"), info, move |name| {
            with_panel(id, |panel| {
                let path = worktree_path(repo.root(), &name);
                if !panel.begin(Task::Worktree, None) {
                    return;
                }
                work::long(
                    move || {
                        repo.add_worktree(&path, &name, None)
                            .map(|tree| tree.root().to_path_buf())
                    },
                    move |result| with_panel(id, |panel| panel.worktree_added(result)),
                );
            });
        });
    }

    fn worktree_added(&self, result: Option<term_git::Result<PathBuf>>) {
        self.end();
        match result {
            Some(Ok(path)) => crate::host::host().open_workspace(&path),
            Some(Err(error)) => self.report(&error),
            None => self.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
        }
        self.start_refresh();
    }

    /// Carries on the merge or rebase once its conflicts are resolved.
    pub(crate) fn continue_operation(&self) {
        let Some(repo) = self.repo() else { return };
        if !self.begin(Task::Continue, None) {
            return;
        }
        let id = self.id();
        work::long(
            move || repo.continue_operation(),
            move |result| with_panel(id, |panel| panel.remote_done(result)),
        );
    }

    /// Asks, then abandons the merge or rebase and puts back the state before it.
    pub(crate) fn abort_operation(&self) {
        let operation = self.read().operation;
        let (Some(operation), Some(window)) = (operation, self.controller().and_then(|c| c.window())) else {
            return;
        };
        let id = self.id();
        sheets::confirm(
            &window,
            super::progress::abort_question(operation),
            t("Git stellt den Stand von vorher wieder her; deine bisherigen Konfliktlösungen gehen verloren."),
            t("Vorgang abbrechen"),
            move || {
                with_panel(id, |panel| {
                    let Some(repo) = panel.repo() else { return };
                    if !panel.begin(Task::Abort, None) {
                        return;
                    }
                    work::long(
                        move || repo.destructive().abort_operation(),
                        move |result| with_panel(id, |panel| panel.remote_done(result)),
                    );
                });
            },
        );
    }

    pub(crate) fn init_repository(&self) {
        let root = self.root().to_path_buf();
        let policy = self.policy();
        let id = self.id();
        work::long(
            move || {
                let git = work::git().ok_or(Error::GitMissing)?;
                git.init(&root, policy).map(drop)
            },
            move |result| {
                with_panel(id, |panel| {
                    if let Some(Err(error)) = &result {
                        panel.state().phase = Phase::Failed(super::text::problem(error).message);
                        panel.show_phase();
                    }
                    panel.start_refresh();
                });
            },
        );
    }

    /// Shows git's or a hook's output of the problem on view.
    pub(crate) fn show_problem_output(&self) {
        let Some(problem) = self.read().problem.clone() else {
            return;
        };
        let (Some(output), Some(window)) = (problem.output, self.controller().and_then(|c| c.window())) else {
            return;
        };
        sheets::output(&window, &problem.message, &output);
    }

    fn update_problem_row(&self) {
        let problem = self.read().problem.clone();
        self.ui().show_problem(problem.as_ref());
    }
}

/// Pushes the branch; one without an upstream goes to `origin`, or the only remote, and gets it as its upstream.
fn push(
    repo: &Repository,
    has_upstream: bool,
    progress: &mut dyn FnMut(&term_git::Progress),
    cancel: &term_git::Cancel,
) -> term_git::Result<()> {
    let mut options = term_git::PushOptions::default();
    if !has_upstream {
        let remotes = repo.remotes()?;
        let remote = remotes
            .iter()
            .find(|r| r.name == "origin")
            .or_else(|| remotes.first())
            .ok_or_else(|| Error::NoUpstream { output: String::new() })?;
        options = term_git::PushOptions {
            remote: Some(remote.name.clone()),
            refspec: Some("HEAD".into()),
            set_upstream: true,
        };
    }
    repo.push(&options, progress, cancel).map(drop)
}

/// A folder for the worktree of `branch` beside the repository at `root`: the repository's name and the branch's, with
/// a number added while the folder is taken.
pub(crate) fn worktree_path(root: &std::path::Path, branch: &str) -> PathBuf {
    let parent = root.parent().unwrap_or(root);
    let repo = root
        .file_name()
        .map_or_else(|| "repo".into(), |name| name.to_string_lossy().into_owned());
    let branch: String = branch
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '.' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = format!("{repo}-{}", branch.trim_matches(['-', '.']));
    (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
        .map(|name| parent.join(name))
        .find(|path| !path.exists())
        .unwrap_or_else(|| parent.join(base))
}

#[cfg(test)]
mod tests {
    use super::worktree_path;

    #[test]
    fn a_worktree_goes_beside_the_repository_under_both_names() {
        let scratch = term_base::testing::scratch("worktree-path");
        let dir = scratch.path();
        let root = dir.join("app");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(worktree_path(&root, "feature/login"), dir.join("app-feature-login"));
        std::fs::create_dir_all(dir.join("app-topic")).unwrap();
        assert_eq!(worktree_path(&root, "topic"), dir.join("app-topic-2"));
    }
}
