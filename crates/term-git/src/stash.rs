//! The stash: its entries with what each changed, parking the working tree's changes, and applying an entry back.
//! An apply that meets conflicts is no merge, rebase, cherry-pick or revert, so git keeps no state for it; this module
//! remembers it in the git folder until the user finishes it or abandons it. Dropping entries and abandoning an
//! apply throw work away and are in [`Destructive`](crate::Destructive).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::changed::{ChangedFile, name_status};
use crate::diff::{Change, FileDiff, diff_args, literal_args, parse};
use crate::error::{Error, Result};
use crate::id::CommitId;
use crate::path::{from_bytes, nul_fields, relative};
use crate::repo::Repository;
use crate::said::{Said, overwritten_paths};

/// The file in the git folder that remembers an apply waiting at its conflicts.
const APPLY_STATE: &str = "term-stash-apply.json";

/// One entry of the stash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// Its place, 0 for the newest; `stash@{index}` names it.
    pub index: usize,
    /// The commit that holds it.
    pub hash: CommitId,
    /// What git wrote for it, such as `On main: message` or `WIP on main: abc1234 subject`.
    pub message: String,
    /// When it was made, in seconds since 1970.
    pub time: i64,
}

/// What a new stash entry takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StashOptions {
    /// Its message; git makes one from the branch and HEAD when empty.
    pub message: String,
    /// Untracked files too, which then leave the working tree.
    pub untracked: bool,
    /// Only what is staged; everything else stays as it is.
    pub staged_only: bool,
}

/// What becomes of a stash entry whose changes are brought back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unstash {
    /// It stays in the stash.
    Apply,
    /// It leaves the stash once its changes are in.
    Pop,
}

/// How applying an entry went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Its changes are in the working tree; a pop removed the entry.
    Clean,
    /// Its changes met conflicts in these files; the entry stays, a pop's too, and [`Repository::stash_apply`] reports
    /// the apply until it is finished or abandoned.
    Conflicts(Vec<PathBuf>),
}

/// An apply that stopped at conflicts and waits for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashApply {
    /// The commit of the entry that was applied.
    pub stash: CommitId,
    pub(crate) message: String,
    /// Whether it was a pop, whose entry git kept because of the conflicts.
    pub pop: bool,
    /// HEAD at the time; an apply whose HEAD moved on is forgotten.
    head: Option<CommitId>,
    /// The index and the tracked files before, as `git stash create` keeps them; none when nothing was changed.
    before: Option<CommitId>,
    /// The untracked files the entry brought into the working tree.
    brought: Vec<PathBuf>,
}

impl Repository {
    /// The entries of the stash, newest first.
    pub fn stashes(&self) -> Result<Vec<Stash>> {
        let output = self
            .git()
            .ok(self.call(["stash", "list", "-z", "--format=%H%x1f%ct%x1f%gs"]))?;
        Ok(nul_fields(&output.stdout)
            .enumerate()
            .filter_map(|(index, record)| {
                let text = String::from_utf8_lossy(record);
                let mut fields = text.splitn(3, '\u{1f}');
                let hash = CommitId::from_git(fields.next()?.trim());
                let time = fields.next()?.parse().unwrap_or(0);
                let message = fields.next().unwrap_or("").to_string();
                (!hash.as_str().is_empty()).then_some(Stash {
                    index,
                    hash,
                    message,
                    time,
                })
            })
            .collect())
    }

    /// Parks the changes `options` names in a new entry and takes them out of the working tree; false when there was
    /// nothing to park.
    pub fn stash(&self, options: &StashOptions) -> Result<bool> {
        let mut args: Vec<OsString> = ["stash", "push", "--quiet"].map(OsString::from).to_vec();
        if options.untracked {
            args.push("--include-untracked".into());
        }
        if options.staged_only {
            args.push("--staged".into());
        }
        let message = options.message.trim();
        if !message.is_empty() {
            args.push("--message".into());
            args.push(message.into());
        }
        let before = self.resolve("refs/stash")?;
        self.git().ok(self.call(args))?;
        Ok(self.resolve("refs/stash")? != before)
    }

    /// Brings the changes of `stash` back into the working tree, keeping or removing the entry as `how` says.
    /// Local changes it would overwrite give `Error::LocalChanges` and leave everything as it was; an entry that is no
    /// longer at its place gives `Error::StashMoved`.
    pub fn apply_stash(&self, stash: &Stash, how: Unstash) -> Result<Applied> {
        let name = self.checked_name(stash)?;
        let state = StashApply {
            stash: stash.hash.clone(),
            message: stash.message.clone(),
            pop: how == Unstash::Pop,
            head: self.head()?,
            before: self.snapshot()?,
            brought: self.untracked_of(&stash.hash)?,
        };
        self.write_apply_state(&state)?;
        let command = match how {
            Unstash::Apply => "apply",
            Unstash::Pop => "pop",
        };
        let output = self
            .git()
            .run(self.call(["stash", command, "--quiet", name.as_str()]).unlimited())?;
        if output.code == 0 {
            self.clear_apply_state()?;
            return Ok(Applied::Clean);
        }
        let conflicts = self.unmerged_paths()?;
        if !conflicts.is_empty() {
            return Ok(Applied::Conflicts(conflicts));
        }
        self.clear_apply_state()?;
        let said = output.said();
        if Said::LocalChanges.in_text(&said) {
            return Err(Error::LocalChanges {
                paths: overwritten_paths(&said),
                output: said,
            });
        }
        Err(crate::run::failure(&output))
    }

    /// The apply that waits at its conflicts, if one does.
    pub fn stash_apply(&self) -> Result<Option<StashApply>> {
        let path = self.git_dir().join(APPLY_STATE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_str::<StashApply>(&text) {
            Ok(state) if state.head == self.head()? => Ok(Some(state)),
            _ => {
                self.clear_apply_state()?;
                Ok(None)
            }
        }
    }

    /// Ends the apply that waits: its conflicts count as resolved, which stages the files as they are now.
    pub fn finish_stash_apply(&self) -> Result<()> {
        let conflicts = self.unmerged_paths()?;
        if !conflicts.is_empty() {
            self.stage(&conflicts)?;
        }
        self.clear_apply_state()
    }

    /// The files `stash` changed against the commit it was made on, its untracked files as added ones.
    pub fn stash_changes(&self, stash: &CommitId) -> Result<Vec<ChangedFile>> {
        let commit = stash.as_str();
        let base = format!("{commit}^1");
        let names = self.git().ok(self.call([
            "diff",
            "--name-status",
            "-z",
            "-M",
            "--no-ext-diff",
            base.as_str(),
            commit,
            "--",
        ]))?;
        let mut files = name_status(&names.stdout);
        files.extend(self.untracked_of(stash)?.into_iter().map(|path| ChangedFile {
            path,
            change: Change::Added,
            from: None,
        }));
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// The diff of `path`, relative to the root, as `stash` changed it; `None` when it did not touch the file.
    pub fn diff_stash(&self, stash: &CommitId, path: &Path) -> Result<Option<FileDiff>> {
        let commit = stash.as_str();
        let path = relative(self.root(), path)?;
        let mut args = diff_args();
        args.extend([
            OsString::from("-M"),
            format!("{commit}^1").into(),
            commit.into(),
            OsString::from("--"),
        ]);
        args.extend(literal_args(std::slice::from_ref(&path)));
        let output = self.git().ok(self.call(args))?;
        if let Some(diff) = parse(&output.stdout)?.into_iter().find(|d| d.path() == path) {
            return Ok(Some(diff));
        }
        let Some(untracked) = self.resolve(&format!("{commit}^3"))? else {
            return Ok(None);
        };
        let mut args = diff_args();
        args.extend([
            self.empty_tree()?.into(),
            untracked.as_str().into(),
            OsString::from("--"),
        ]);
        args.extend(literal_args(std::slice::from_ref(&path)));
        let output = self.git().ok(self.call(args))?;
        Ok(parse(&output.stdout)?.into_iter().find(|d| d.path() == path))
    }

    /// `stash@{n}` for `stash`, once it is sure the entry is still at its place.
    pub(crate) fn checked_name(&self, stash: &Stash) -> Result<String> {
        let name = format!("stash@{{{}}}", stash.index);
        if self.resolve(&name)?.as_ref() != Some(&stash.hash) {
            return Err(Error::StashMoved);
        }
        Ok(name)
    }

    /// Puts back the working tree as it was before the apply that waits, and forgets the apply.
    pub(crate) fn abort_stash_apply_with(&self) -> Result<()> {
        let Some(state) = self.stash_apply()? else {
            return Ok(());
        };
        self.git().ok(self.call(["reset", "--hard", "--quiet", "HEAD", "--"]))?;
        for path in &state.brought {
            // The record lies in the git folder, so a path in it that leads outside the working tree is left alone.
            let Ok(inside) = relative(self.root(), path) else {
                continue;
            };
            match std::fs::remove_file(self.root().join(inside)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        if let Some(before) = &state.before {
            self.git()
                .ok(self.call(["stash", "apply", "--index", "--quiet", before.as_str()]))?;
        }
        self.clear_apply_state()
    }

    /// The index and tracked files as a commit that no ref keeps, or `None` when they match HEAD.
    fn snapshot(&self) -> Result<Option<CommitId>> {
        if self.head()?.is_none() {
            return Ok(None);
        }
        let hash = self.git().ok(self.call(["stash", "create"]))?.text();
        Ok((!hash.is_empty()).then(|| CommitId::from_git(hash)))
    }

    /// The untracked files kept in the stash commit `stash`, which git stores as its third parent.
    fn untracked_of(&self, stash: &CommitId) -> Result<Vec<PathBuf>> {
        let Some(tree) = self.resolve(&format!("{stash}^3"))? else {
            return Ok(Vec::new());
        };
        let output = self
            .git()
            .ok(self.call(["ls-tree", "-r", "-z", "--name-only", tree.as_str()]))?;
        Ok(nul_fields(&output.stdout).map(from_bytes).collect())
    }

    fn write_apply_state(&self, state: &StashApply) -> Result<()> {
        let text = serde_json::to_string(state).map_err(|e| Error::Unexpected { what: e.to_string() })?;
        std::fs::write(self.git_dir().join(APPLY_STATE), text)?;
        Ok(())
    }

    fn clear_apply_state(&self) -> Result<()> {
        match std::fs::remove_file(self.git_dir().join(APPLY_STATE)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_apply_is_stored_as_before() {
        let stored = serde_json::json!({
            "stash": "1111111111111111111111111111111111111111",
            "message": "On main: wip",
            "pop": true,
            "head": "2222222222222222222222222222222222222222",
            "before": null,
            "brought": ["new.txt"],
        });
        let state: StashApply = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(state.stash.as_str(), "1111111111111111111111111111111111111111");
        assert_eq!(
            state.head.as_ref().map(CommitId::as_str),
            Some("2222222222222222222222222222222222222222")
        );
        assert_eq!(serde_json::to_value(&state).unwrap(), stored);
    }
}
