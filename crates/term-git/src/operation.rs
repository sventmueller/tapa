//! A merge, rebase, cherry-pick or revert that stopped at conflicts and waits for the user, and carrying it on.
//! Abandoning it is [`Destructive::abort_operation`](crate::Destructive::abort_operation).

use crate::error::{Error, Result};
use crate::repo::Repository;

/// An operation git stopped in the middle of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// A merge waits for its commit.
    Merge,
    /// A rebase stopped at a commit.
    Rebase,
    /// A cherry-pick stopped at a commit.
    CherryPick,
    /// A revert stopped at a commit.
    Revert,
}

impl Operation {
    /// The git command that runs it, such as `rebase`.
    fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }
}

impl Repository {
    /// The operation this working tree is in the middle of; `None` also for `git am`, which the app does not drive.
    pub fn operation(&self) -> Result<Option<Operation>> {
        let dir = self.git_dir();
        if dir.join("rebase-merge").is_dir() {
            return Ok(Some(Operation::Rebase));
        }
        if dir.join("rebase-apply").is_dir() {
            let am = dir.join("rebase-apply/applying").exists();
            return Ok((!am).then_some(Operation::Rebase));
        }
        for (head, operation) in [
            ("MERGE_HEAD", Operation::Merge),
            ("CHERRY_PICK_HEAD", Operation::CherryPick),
            ("REVERT_HEAD", Operation::Revert),
        ] {
            if self.resolve(head)?.is_some() {
                return Ok(Some(operation));
            }
        }
        Ok(None)
    }

    /// Carries the operation on once every conflict is resolved and staged; a merge commits with the message git
    /// prepared. Hooks run as for a commit. A rebase that stops at the next commit's conflicts gives
    /// `Error::Conflicts` again.
    pub fn continue_operation(&self) -> Result<()> {
        let Some(operation) = self.operation()? else {
            return Ok(());
        };
        let unresolved = self.unmerged_paths()?;
        if !unresolved.is_empty() {
            return Err(Error::Conflicts {
                paths: unresolved,
                output: String::new(),
            });
        }
        let args = match operation {
            Operation::Merge => vec!["commit", "--no-edit"],
            other => vec![other.command(), "--continue"],
        };
        let (output, failed_hook) = crate::hooks::run(self.git(), self.call(args).unlimited())?;
        if output.code == 0 {
            return Ok(());
        }
        if let Some(hook) = failed_hook {
            return Err(Error::HookFailed {
                hook,
                output: output.said(),
            });
        }
        let conflicts = self.unmerged_paths()?;
        if !conflicts.is_empty() {
            return Err(Error::Conflicts {
                paths: conflicts,
                output: output.said(),
            });
        }
        Err(crate::run::failure(&output))
    }

    /// Abandons the operation and puts back the state before it began.
    pub(crate) fn abort_operation_with(&self) -> Result<()> {
        let Some(operation) = self.operation()? else {
            return Ok(());
        };
        self.git()
            .ok(self.call([operation.command(), "--abort"]).unlimited())
            .map(drop)
    }
}
