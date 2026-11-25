//! Finding the repository a folder belongs to.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::id::CommitId;
use crate::neutral::Policy;
use crate::run::{Call, Git};

/// A git repository with a working tree, as seen from one of its worktrees.
#[derive(Debug, Clone)]
pub struct Repository {
    git: Git,
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    policy: Policy,
}

impl Git {
    /// The repository `path` lies in, run under `policy` from here on; `path` may be a file or a folder anywhere
    /// inside the working tree.
    pub fn discover(&self, path: &Path, policy: Policy) -> Result<Repository> {
        let dir = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        if !dir.is_dir() {
            return Err(Error::NotARepository {
                path: path.to_path_buf(),
            });
        }
        let output = self.run(Call::new(
            dir,
            [
                "rev-parse",
                "--path-format=absolute",
                "--is-bare-repository",
                "--git-dir",
                "--git-common-dir",
            ],
            policy,
        ))?;
        if output.code != 0 {
            return Err(match crate::run::failure(&output) {
                Error::Failed { .. } => Error::NotARepository {
                    path: path.to_path_buf(),
                },
                other => other,
            });
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut lines = text.lines();
        let (Some(bare), Some(git_dir), Some(common_dir)) = (lines.next(), lines.next(), lines.next()) else {
            return Err(Error::Unexpected { what: text });
        };
        if bare == "true" {
            return Err(Error::BareRepository {
                path: PathBuf::from(git_dir),
            });
        }
        let top = self.run(Call::new(
            dir,
            ["rev-parse", "--path-format=absolute", "--show-toplevel"],
            policy,
        ))?;
        if top.code != 0 || top.text().is_empty() {
            return Err(Error::NotARepository {
                path: path.to_path_buf(),
            });
        }
        Ok(Repository {
            git: self.clone(),
            root: PathBuf::from(top.text()),
            git_dir: PathBuf::from(git_dir),
            common_dir: PathBuf::from(common_dir),
            policy,
        })
    }

    /// Makes the existing folder `folder` a new repository with git's default branch name, and returns it under
    /// `policy`.
    pub fn init(&self, folder: &Path, policy: Policy) -> Result<Repository> {
        if !folder.is_dir() {
            return Err(Error::NotARepository {
                path: folder.to_path_buf(),
            });
        }
        self.ok(Call::new(folder, ["init", "--quiet"], policy))?;
        self.discover(folder, policy)
    }
}

impl Repository {
    /// The top folder of this working tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This working tree's git folder: `.git`, or for a linked worktree its folder under `.git/worktrees`.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// The git folder all worktrees of the repository share.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// Whether this working tree was added with `git worktree add`.
    pub fn is_linked_worktree(&self) -> bool {
        self.git_dir != self.common_dir
    }

    /// Whether git here runs the repository's hooks, filters and helpers.
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The git this repository runs.
    pub(crate) fn git(&self) -> &Git {
        &self.git
    }

    /// The commit HEAD points at; `None` on a branch without commits.
    pub fn head(&self) -> Result<Option<CommitId>> {
        self.resolve("HEAD")
    }

    /// The commit `revision` names, or `None` when it names none.
    pub fn resolve(&self, revision: &str) -> Result<Option<CommitId>> {
        let spec = format!("{revision}^{{commit}}");
        let output = self
            .git
            .run(self.call(["rev-parse", "--verify", "--quiet", "--end-of-options", &spec]))?;
        Ok((output.code == 0).then(|| CommitId::from_git(output.text())))
    }

    /// The tree without entries, whose name depends on the repository's hash.
    pub(crate) fn empty_tree(&self) -> Result<String> {
        Ok(self
            .git
            .ok(self.call(["hash-object", "-t", "tree", "/dev/null"]))?
            .text())
    }

    /// `git <args>` at the root.
    pub(crate) fn call<I, S>(&self, args: I) -> Call<'_>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Call::new(&self.root, args, self.policy)
    }
}
