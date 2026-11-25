//! The Git panel's controller for what it shows: building it for a workspace, reading git on the git lane when the
//! folder changed, handing the result to the model and bringing the views in line.

use std::path::Path;
use std::time::Duration;

use term_git::{Policy, StatusOptions};

use super::model::{Loaded, Phase, RowKey, Section, Start, StashRead};
use super::panel::{GitPanel, with_panel};
use super::text::{Problem, problem};
use super::work;
use term_ui::kit::main_after;
use term_ui::workspace_host::{Trust, Workspace};

/// How long changes on disk gather before the panel reads git again.
const DEBOUNCE: Duration = Duration::from_millis(300);

/// How often the ages of the commits on view are said again.
const AGE_INTERVAL: Duration = Duration::from_secs(60);

impl GitPanel {
    /// The panel of `workspace`, built for its base path the first time and reading the repository from then on.
    pub(crate) fn attach(workspace: &Workspace) -> Option<objc2::rc::Retained<GitPanel>> {
        if let Some(panel) = crate::host::state().panels.of(*workspace) {
            return Some(panel);
        }
        let root = workspace.project_root()?;
        let root = root.canonicalize().unwrap_or(root);
        let panel = GitPanel::new(workspace.mtm(), workspace, root);
        panel.update_placeholder();
        crate::host::state().panels.insert(panel.id(), panel.clone());
        panel.start_refresh();
        panel.schedule_ages();
        Some(panel)
    }

    /// How git may treat the workspace's repository: neutralised unless the user trusts the workspace (E21, E22).
    pub(crate) fn policy(&self) -> Policy {
        self.controller()
            .map_or(Policy::Untrusted, |workspace| policy_of(&workspace))
    }

    /// The trust in the workspace changed: the repository is found again under the new policy before git runs there.
    pub(crate) fn trust_changed(&self) {
        let mut state = self.state();
        if state.repo.as_ref().is_some_and(|repo| repo.policy() != self.policy()) {
            state.repo = None;
        }
        drop(state);
        self.ui().untrusted.setHidden(self.policy() == Policy::Trusted);
        self.start_refresh();
    }

    /// Says the ages of the commits again every `AGE_INTERVAL` while the panel shows, until its workspace closes.
    fn schedule_ages(&self) {
        let id = self.id();
        main_after(AGE_INTERVAL, move || {
            with_panel(id, |panel| {
                panel.refresh_ages();
                panel.schedule_ages();
            });
        });
    }

    /// Reloads the commit rows, whose views compute their age when they are made.
    pub(crate) fn refresh_ages(&self) {
        let page = &self.ui().page;
        let showing = page.window().is_some_and(|w| w.isVisible()) && !page.isHiddenOrHasHiddenAncestor();
        if !showing || self.read().model.commits.is_empty() {
            return;
        }
        let item = self.item_for(&RowKey::Section(Section::Commits));
        // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
        unsafe { self.ui().outline.reloadItem_reloadChildren(Some(&item), true) };
    }

    /// Reads git again once changes on disk have settled.
    pub(crate) fn schedule_refresh(&self) {
        if self.state().schedule_refresh() {
            let id = self.id();
            main_after(DEBOUNCE, move || with_panel(id, GitPanel::start_refresh));
        }
    }

    /// Reads git now, or right after the read that is running.
    pub(crate) fn start_refresh(&self) {
        let start = self.state().start_refresh();
        let limit = match start {
            Start::Queued => return,
            Start::Read { limit } => limit,
        };
        let root = self.root().to_path_buf();
        let policy = self.policy();
        let id = self.id();
        let queued = work::local(
            move || load(&root, policy, limit),
            move |loaded| with_panel(id, |p| p.loaded(loaded)),
        );
        if !queued {
            self.state().refresh_not_started();
            self.schedule_refresh();
        }
    }

    fn loaded(&self, loaded: Option<Loaded>) {
        let again = self.state().refresh_finished();
        let loaded =
            loaded.unwrap_or_else(|| Loaded::Failed(Problem::plain(splat_i18n::t("Git ist unerwartet abgebrochen."))));
        let changed = self.state().apply(loaded);
        if changed {
            self.show_phase();
        }
        if again {
            self.schedule_refresh();
        }
        super::diff::refresh_for(self.root());
    }

    /// Puts every view in line with the state.
    pub(crate) fn show_phase(&self) {
        let (phase, count) = {
            let state = self.read();
            (state.phase.clone(), state.model.changed_files())
        };
        self.ui().show_phase(self, &phase);
        self.ui().untrusted.setHidden(self.policy() == Policy::Trusted);
        if phase == Phase::Ready {
            self.update_header();
            self.reload_outline();
            self.update_busy();
            self.update_banner();
            self.update_problem();
        }
        self.refresh_toolbar();
        if let Some(controller) = self.controller() {
            crate::host::host().set_change_count(controller.id(), count);
        }
    }

    /// Shows `problem` above the list, or hides it.
    pub(crate) fn set_problem(&self, problem: Option<Problem>) {
        self.state().problem = problem;
        self.update_problem();
    }

    fn update_problem(&self) {
        let problem = self.read().problem.clone();
        self.ui().show_problem(problem.as_ref());
    }

    pub(crate) fn report(&self, error: &term_git::Error) {
        if matches!(error, term_git::Error::Cancelled) {
            return;
        }
        let problem = problem(error);
        let hook = problem.hook;
        self.set_problem(Some(problem));
        if hook {
            self.show_problem_output();
        }
    }
}

/// Everything the panel shows, with up to `limit` commits, read on the git lane under `policy`.
fn load(root: &Path, policy: Policy, limit: usize) -> Loaded {
    let Some(git) = work::git() else { return Loaded::NoGit };
    if git.program().is_none() {
        return Loaded::NoGit;
    }
    let repo = match git.discover(root, policy) {
        Ok(repo) => repo,
        Err(term_git::Error::NotARepository { .. }) => return Loaded::NotRepository,
        Err(term_git::Error::GitMissing) => return Loaded::NoGit,
        Err(error) => return Loaded::Failed(problem(&error)),
    };
    let status = match repo.status(StatusOptions::default()) {
        Ok(status) => status,
        Err(error) => return Loaded::Failed(problem(&error)),
    };
    let branches = repo.branches().unwrap_or_default();
    let log = repo
        .history(limit, status.branch.upstream.as_deref())
        .unwrap_or_default();
    let operation = repo.operation().unwrap_or(None);
    let stash = Box::new(StashRead {
        entries: repo.stashes().unwrap_or_default(),
        apply: repo.stash_apply().unwrap_or(None),
    });
    Loaded::Ready {
        repo,
        status: Box::new(status),
        branches,
        log,
        operation,
        stash,
    }
}

/// How git may treat the repositories of `workspace`: neutralised unless the user trusts it (E21, E22); a closed
/// workspace counts as untrusted.
pub(crate) fn policy_of(workspace: &Workspace) -> Policy {
    if workspace.load().is_some_and(|w| w.is_trusted()) {
        Policy::Trusted
    } else {
        Policy::Untrusted
    }
}
