//! Commits, with the repository's hooks running as they would in a terminal.

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::id::CommitId;
use crate::repo::Repository;
use crate::run::Output;
use crate::said::Said;
use crate::status::{EntryKind, StatusOptions, Untracked};

/// How to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommitOptions {
    /// Replace the last commit; an empty message keeps its message.
    pub amend: bool,
    /// Add a `Signed-off-by` trailer.
    pub signoff: bool,
}

impl Repository {
    /// Commits what is staged and returns the new commit's hash. Empty commits are refused; hooks run, and one that
    /// refuses gives `Error::HookFailed` with what it printed. There is no time limit, since hooks may run tests; the
    /// activity view can stop the process.
    pub fn commit(&self, message: &str, options: CommitOptions) -> Result<CommitId> {
        let keep_message = options.amend && message.trim().is_empty();
        if message.trim().is_empty() && !keep_message {
            return Err(Error::EmptyMessage);
        }
        if !options.amend && !self.merging()? && !self.has_staged_changes()? {
            return Err(Error::NothingToCommit);
        }
        let mut call = self.call(["commit", "--quiet"]).unlimited();
        if options.amend {
            call = call.args(["--amend"]);
        }
        if options.signoff {
            call = call.args(["--signoff"]);
        }
        call = if keep_message {
            call.args(["--no-edit"])
        } else {
            call.args(["--file=-"]).input(message.as_bytes().to_vec())
        };
        let (output, failed_hook) = crate::hooks::run(self.git(), call)?;
        if output.code != 0 {
            return Err(self.commit_failure(&output, failed_hook, options.amend)?);
        }
        self.head()?.ok_or_else(|| Error::Unexpected {
            what: "no HEAD after commit".into(),
        })
    }

    fn commit_failure(&self, output: &Output, failed_hook: Option<String>, amend: bool) -> Result<Error> {
        if let Some(hook) = failed_hook {
            return Ok(Error::HookFailed {
                hook,
                output: output.said(),
            });
        }
        let said = output.said();
        if !amend && !self.merging()? && !self.has_staged_changes()? {
            return Ok(Error::NothingToCommit);
        }
        if Said::EmptyMessage.in_text(&said) {
            return Ok(Error::EmptyMessage);
        }
        let paths = self.unmerged_paths()?;
        if !paths.is_empty() {
            return Ok(Error::Conflicts { paths, output: said });
        }
        Ok(crate::run::failure(output))
    }

    /// Whether the index differs from HEAD.
    pub(crate) fn has_staged_changes(&self) -> Result<bool> {
        let output = self
            .git()
            .run(self.call(["diff", "--cached", "--quiet", "--ignore-submodules=none"]))?;
        match output.code {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(crate::run::failure(&output)),
        }
    }

    /// Whether a merge waits for its commit, which may then be empty.
    fn merging(&self) -> Result<bool> {
        Ok(self.resolve("MERGE_HEAD")?.is_some())
    }

    /// The files with merge conflicts.
    pub(crate) fn unmerged_paths(&self) -> Result<Vec<PathBuf>> {
        let status = self.status(StatusOptions {
            untracked: Untracked::No,
            ignored: false,
        })?;
        Ok(status
            .entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path)
            .collect())
    }
}
