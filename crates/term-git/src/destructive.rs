//! Operations that throw work away for good. The app asks the user before each one, whatever rules allow.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::path::{from_bytes, unquoted};
use crate::remote::{Progress, PushOptions, PushOutcome, word};
use crate::repo::Repository;
use crate::run::Cancel;
use crate::stash::Stash;

/// The destructive operations on a repository; every call site needs the user's explicit approval.
#[derive(Debug, Clone, Copy)]
pub struct Destructive<'a> {
    repository: &'a Repository,
}

impl Repository {
    /// The operations that need the user's approval each time.
    pub fn destructive(&self) -> Destructive<'_> {
        Destructive { repository: self }
    }
}

impl Destructive<'_> {
    /// Pushes over the remote's commits with `--force-with-lease`, which still fails when the remote moved since the
    /// last fetch.
    pub fn force_push(
        &self,
        options: &PushOptions,
        progress: &mut dyn FnMut(&Progress),
        cancel: &Cancel,
    ) -> Result<PushOutcome> {
        self.repository.push_with(options, true, progress, cancel)
    }

    /// Moves the current branch to `target` and makes index and working tree match it, dropping every change to
    /// tracked files.
    pub fn reset_hard(&self, target: &str) -> Result<()> {
        let repository = self.repository;
        repository
            .git()
            .ok(repository.call(["reset", "--hard", "--quiet", word(target)?, "--"]))
            .map(drop)
    }

    /// What [`Destructive::clean`] would delete; deletes nothing, so it needs no approval.
    pub fn clean_preview(&self) -> Result<Vec<PathBuf>> {
        self.clean_with("-n", "Would remove ")
    }

    /// Deletes untracked files and folders that are not ignored, and returns them.
    pub fn clean(&self) -> Result<Vec<PathBuf>> {
        self.clean_with("-f", "Removing ")
    }

    /// Deletes the branch `name` even when its commits are not merged anywhere.
    pub fn delete_branch(&self, name: &str) -> Result<()> {
        let repository = self.repository;
        repository
            .git()
            .ok(repository.call(["branch", "--delete", "--force", word(name)?]))
            .map(drop)
    }

    /// Abandons the merge, rebase, cherry-pick or revert the working tree is in the middle of, dropping the conflict
    /// resolutions made so far.
    pub fn abort_operation(&self) -> Result<()> {
        self.repository.abort_operation_with()
    }

    /// Removes the working tree at `path` with its changes and untracked files.
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        self.repository.remove_worktree_with(path, true)
    }

    /// Deletes the stash entry `stash`; `Error::StashMoved` when it is no longer at its place.
    pub fn drop_stash(&self, stash: &Stash) -> Result<()> {
        let repository = self.repository;
        let name = repository.checked_name(stash)?;
        repository
            .git()
            .ok(repository.call(["stash", "drop", "--quiet", name.as_str()]))
            .map(drop)
    }

    /// Deletes every stash entry.
    pub fn clear_stashes(&self) -> Result<()> {
        let repository = self.repository;
        repository.git().ok(repository.call(["stash", "clear"])).map(drop)
    }

    /// Abandons the stash apply that waits at its conflicts and puts back the index and working tree from before it,
    /// dropping the conflict resolutions made so far; the entry stays in the stash.
    pub fn abort_stash_apply(&self) -> Result<()> {
        self.repository.abort_stash_apply_with()
    }

    fn clean_with(&self, mode: &str, prefix: &str) -> Result<Vec<PathBuf>> {
        let repository = self.repository;
        let output = repository
            .git()
            .ok(repository.call(["-c", "core.quotepath=false", "clean", mode, "-d"]))?;
        Ok(output
            .stdout
            .split(|&b| b == b'\n')
            .filter_map(|line| line.strip_prefix(prefix.as_bytes()))
            .map(|path| from_bytes(&unquoted(path)))
            .collect())
    }
}
