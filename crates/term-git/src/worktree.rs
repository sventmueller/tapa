//! Worktrees, so that work on another branch can go on beside the user's.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::id::CommitId;
use crate::path::from_bytes;
use crate::remote::word;
use crate::repo::Repository;

/// A working tree of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Its folder.
    pub path: PathBuf,
    /// The commit it has checked out; `None` before the first commit.
    pub head: Option<CommitId>,
    /// Its branch's full ref; `None` when detached.
    pub branch: Option<String>,
    /// Whether it is the repository's main working tree.
    pub main: bool,
    /// Why it is locked, when it is; an empty reason when none was given.
    pub(crate) locked: Option<String>,
    /// Why git could prune it, such as a folder that is gone.
    pub prunable: Option<String>,
}

impl Repository {
    /// The repository's working trees, the main one first.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let output = self.git().ok(self.call(["worktree", "list", "--porcelain", "-z"]))?;
        Ok(parse(&output.stdout))
    }

    /// Adds a working tree at `path` on a new branch `branch` that starts at `start`, or at HEAD, and returns it under
    /// this repository's policy.
    pub fn add_worktree(&self, path: &Path, branch: &str, start: Option<&str>) -> Result<Repository> {
        self.check_branch_name(branch)?;
        let mut args: Vec<OsString> = ["worktree", "add", "-b", branch, "--"].map(OsString::from).to_vec();
        args.push(path.as_os_str().to_os_string());
        if let Some(start) = start {
            args.push(word(start)?.into());
        }
        let (output, failed_hook) = crate::hooks::run(self.git(), self.call(args).unlimited())?;
        if output.code != 0 {
            return Err(match failed_hook {
                Some(hook) => Error::HookFailed {
                    hook,
                    output: output.said(),
                },
                None => crate::run::failure(&output),
            });
        }
        self.git().discover(path, self.policy())
    }

    /// Removes the working tree at `path`; one with changes or untracked files gives `Error::LocalChanges`, and
    /// removing it anyway is [`Destructive::remove_worktree`](crate::Destructive::remove_worktree).
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        self.remove_worktree_with(path, false)
    }

    /// Forgets working trees whose folders are gone.
    pub fn prune_worktrees(&self) -> Result<()> {
        self.git().ok(self.call(["worktree", "prune"])).map(drop)
    }

    pub(crate) fn remove_worktree_with(&self, path: &Path, force: bool) -> Result<()> {
        let mut args: Vec<OsString> = ["worktree", "remove"].map(OsString::from).to_vec();
        if force {
            args.push("--force".into());
        }
        args.push("--".into());
        args.push(path.as_os_str().to_os_string());
        let output = self.git().run(self.call(args))?;
        if output.code == 0 {
            return Ok(());
        }
        if !force && !self.locked(path)? && self.has_changes(path)? {
            return Err(Error::LocalChanges {
                paths: Vec::new(),
                output: output.said(),
            });
        }
        Err(crate::run::failure(&output))
    }

    /// Whether the working tree at `path` is locked, which git checks before its changes.
    fn locked(&self, path: &Path) -> Result<bool> {
        let same = |other: &Path| match (path.canonicalize(), other.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => path == other,
        };
        Ok(self.worktrees()?.iter().any(|w| w.locked.is_some() && same(&w.path)))
    }

    /// Whether the working tree at `path` has changes or untracked files, as `git worktree remove` counts them.
    fn has_changes(&self, path: &Path) -> Result<bool> {
        let call = crate::run::Call::new(
            path,
            ["status", "--porcelain", "-z", "--ignore-submodules=none"],
            self.policy(),
        );
        let output = self.git().run(call)?;
        Ok(output.code == 0 && !output.stdout.is_empty())
    }
}

fn parse(output: &[u8]) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in output.split(|&b| b == 0) {
        if field.is_empty() {
            worktrees.extend(current.take());
            continue;
        }
        let (key, value) = match field.iter().position(|&b| b == b' ') {
            Some(space) => (&field[..space], Some(&field[space + 1..])),
            None => (field, None),
        };
        let text = || value.map(|v| String::from_utf8_lossy(v).into_owned());
        if key == b"worktree" {
            worktrees.extend(current.take());
            current = Some(Worktree {
                path: from_bytes(value.unwrap_or_default()),
                head: None,
                branch: None,
                main: worktrees.is_empty(),
                locked: None,
                prunable: None,
            });
            continue;
        }
        let Some(worktree) = current.as_mut() else { continue };
        match key {
            b"HEAD" => worktree.head = text().filter(|h| h.bytes().any(|b| b != b'0')).map(CommitId::from_git),
            b"branch" => worktree.branch = text(),
            b"locked" => worktree.locked = Some(text().unwrap_or_default()),
            b"prunable" => worktree.prunable = Some(text().unwrap_or_default()),
            _ => {}
        }
    }
    worktrees.extend(current);
    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktrees_are_read_from_nul_separated_records() {
        let output = b"worktree /r\0HEAD 0a85\0branch refs/heads/main\0\0\
worktree /w t\0HEAD 0a85\0detached\0locked\0\0\
worktree /gone\0HEAD 0a85\0branch refs/heads/x\0prunable gitdir file points to non-existent location\0\0";
        let worktrees = parse(output);
        assert_eq!(worktrees.len(), 3);
        assert!(worktrees[0].main);
        assert_eq!(worktrees[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(worktrees[1].path, PathBuf::from("/w t"));
        assert!(!worktrees[1].main);
        assert_eq!(worktrees[1].branch, None);
        assert_eq!(worktrees[1].locked.as_deref(), Some(""));
        assert!(worktrees[2].prunable.as_deref().unwrap().contains("non-existent"));
    }
}
