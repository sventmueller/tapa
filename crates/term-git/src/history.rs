//! The history as a graph: commits of HEAD and its upstream with their full messages, and what a single commit
//! changed.

use std::ffi::OsString;
use std::path::Path;

use crate::branch::{Commit, parse_commit};
use crate::changed::{ChangedFile, name_status};
use crate::diff::{FileDiff, diff_args, literal_args, parse};
use crate::error::Result;
use crate::id::CommitId;
use crate::path::nul_fields;
use crate::remote::word;
use crate::repo::Repository;

/// What a commit changed against its first parent, or against nothing for a root commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitChanges {
    /// The files, sorted by path.
    pub files: Vec<ChangedFile>,
    /// Lines added across the text files.
    pub insertions: usize,
    /// Lines removed across the text files.
    pub deletions: usize,
}

impl Repository {
    /// The last `limit` commits of HEAD and of `also`, such as its upstream, children always before their parents so
    /// the lines of a graph never run upward; empty before the first commit.
    pub fn history(&self, limit: usize, also: Option<&str>) -> Result<Vec<Commit>> {
        if self.head()?.is_none() {
            return Ok(Vec::new());
        }
        let count = format!("--max-count={limit}");
        let mut args: Vec<OsString> = [
            "-c",
            "log.showSignature=false",
            "log",
            "-z",
            "--no-color",
            "--topo-order",
            "--decorate=short",
            &count,
            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%D%x1f%s%x1f%b",
            "HEAD",
        ]
        .map(OsString::from)
        .to_vec();
        if let Some(other) = also
            && let Some(commit) = self.resolve(word(other)?)?
        {
            args.push(commit.as_str().into());
        }
        args.push("--".into());
        let output = self.git().ok(self.call(args))?;
        Ok(nul_fields(&output.stdout).filter_map(parse_commit).collect())
    }

    /// The files `commit` changed and how many lines.
    pub fn commit_changes(&self, commit: &CommitId) -> Result<CommitChanges> {
        let base = self.base_of(commit)?;
        let commit = commit.as_str();
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
        let numbers = self.git().ok(self.call([
            "diff",
            "--numstat",
            "-z",
            "-M",
            "--no-ext-diff",
            base.as_str(),
            commit,
            "--",
        ]))?;
        let (insertions, deletions) = line_counts(&numbers.stdout);
        let mut files = name_status(&names.stdout);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(CommitChanges {
            files,
            insertions,
            deletions,
        })
    }

    /// The diff of `path`, relative to the root, as `commit` changed it; `None` when it did not touch the file.
    pub fn diff_commit(&self, commit: &CommitId, path: &Path) -> Result<Option<FileDiff>> {
        let base = self.base_of(commit)?;
        let mut args = diff_args();
        args.extend([
            OsString::from("-M"),
            base.into(),
            commit.as_str().into(),
            OsString::from("--"),
        ]);
        args.extend(literal_args(&[path.to_path_buf()]));
        let output = self.git().ok(self.call(args))?;
        Ok(parse(&output.stdout)?.into_iter().find(|d| d.path() == path))
    }

    /// The first parent of `commit`, or the empty tree for a root commit.
    fn base_of(&self, commit: &CommitId) -> Result<String> {
        let output = self
            .git()
            .ok(self.call(["rev-list", "--parents", "--max-count=1", commit.as_str(), "--"]))?;
        match output.text().split_whitespace().nth(1) {
            Some(parent) => Ok(parent.to_string()),
            None => self.empty_tree(),
        }
    }
}

/// The added and removed lines of `git diff --numstat -z`; binary files, counted as `-`, add nothing. A rename has an
/// empty path field followed by both paths.
fn line_counts(output: &[u8]) -> (usize, usize) {
    let mut fields = output.split(|&b| b == 0);
    let (mut added, mut removed) = (0, 0);
    while let Some(field) = fields.next() {
        let text = String::from_utf8_lossy(field);
        let mut parts = text.splitn(3, '\t');
        let (Some(plus), Some(minus), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        added += plus.parse::<usize>().unwrap_or(0);
        removed += minus.parse::<usize>().unwrap_or(0);
        if path.is_empty() {
            fields.next();
            fields.next();
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_counts_add_up_text_files_and_skip_both_paths_of_a_rename() {
        let output = b"3\t1\tsrc/a.rs\0-\t-\tlogo.png\x002\t0\t\0old.rs\0new.rs\x0010\t4\tb.rs\0";
        assert_eq!(line_counts(output), (15, 5));
        assert_eq!(line_counts(b""), (0, 0));
    }
}
