//! Staging, unstaging and discarding whole files or single hunks. Hunks travel as patches through `git apply`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::diff::{Change, FileDiff, Hunk, LineKind, literal_args};
use crate::error::{Error, Result};
use crate::path::{all_relative, bytes, from_bytes, literal_pathspecs, nul_fields, quoted};
use crate::repo::Repository;
use crate::run::{Call, Git};

impl Repository {
    /// Stages `paths` as they are in the working tree, deletions included.
    pub fn stage<P: AsRef<Path>>(&self, paths: &[P]) -> Result<()> {
        self.with_paths(&["add", "--all"], paths)
    }

    /// Stages every change, untracked files included.
    pub fn stage_all(&self) -> Result<()> {
        self.git().ok(self.call(["add", "--all"])).map(drop)
    }

    /// Takes `paths` out of the index back to HEAD's version, leaving the working tree alone.
    pub fn unstage<P: AsRef<Path>>(&self, paths: &[P]) -> Result<()> {
        self.with_paths(&["reset", "--quiet"], paths)
    }

    /// Unstages everything.
    pub fn unstage_all(&self) -> Result<()> {
        self.git().ok(self.call(["reset", "--quiet"])).map(drop)
    }

    /// Puts the staged version of `paths` back into the working tree, dropping what is not staged. Untracked files
    /// are refused with `Error::Untracked` before anything changes: deleting them is final, so the app moves them to
    /// the Trash instead.
    pub fn discard<P: AsRef<Path>>(&self, paths: &[P]) -> Result<()> {
        let paths = all_relative(self.root(), paths)?;
        if paths.is_empty() {
            return Ok(());
        }
        let mut args: Vec<OsString> = ["ls-files", "-z", "--others", "--exclude-standard", "--"]
            .map(OsString::from)
            .to_vec();
        args.extend(literal_args(&paths));
        let output = self.git().ok(self.call(args))?;
        let untracked: Vec<PathBuf> = nul_fields(&output.stdout).map(from_bytes).collect();
        if !untracked.is_empty() {
            return Err(Error::Untracked { paths: untracked });
        }
        self.with_paths(&["restore", "--worktree"], &paths)
    }

    /// Stages the hunks of `diff` at `hunks`; `diff` is a [`DiffSource::Unstaged`](crate::DiffSource) diff.
    pub fn stage_hunks(&self, diff: &FileDiff, hunks: &[usize]) -> Result<()> {
        apply_hunks(self.git(), self.call(["apply", "--cached"]), diff, hunks, false)
    }

    /// Unstages the hunks of `diff` at `hunks`; `diff` is a [`DiffSource::Staged`](crate::DiffSource) diff.
    pub fn unstage_hunks(&self, diff: &FileDiff, hunks: &[usize]) -> Result<()> {
        apply_hunks(self.git(), self.call(["apply", "--cached"]), diff, hunks, true)
    }

    /// Drops the hunks of `diff` at `hunks` from the working tree; `diff` is a
    /// [`DiffSource::Unstaged`](crate::DiffSource) diff. A new file is refused as in [`Repository::discard`].
    pub fn discard_hunks(&self, diff: &FileDiff, hunks: &[usize]) -> Result<()> {
        if diff.change == Change::Added {
            return Err(Error::Untracked {
                paths: vec![diff.path().to_path_buf()],
            });
        }
        apply_hunks(self.git(), self.call(["apply"]), diff, hunks, true)
    }

    fn with_paths<P: AsRef<Path>>(&self, command: &[&str], paths: &[P]) -> Result<()> {
        let paths = all_relative(self.root(), paths)?;
        if paths.is_empty() {
            return Ok(());
        }
        let call = self
            .call(command)
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .input(literal_pathspecs(&paths));
        self.git().ok(call).map(drop)
    }
}

/// Applies the hunks of `diff` at `hunks` with `call`, a `git apply` that says where; backwards when `reverse`.
pub(crate) fn apply_hunks(git: &Git, call: Call<'_>, diff: &FileDiff, hunks: &[usize], reverse: bool) -> Result<()> {
    if diff.binary {
        return Err(Error::Binary {
            path: diff.path().to_path_buf(),
        });
    }
    let selected: Vec<&Hunk> = hunks.iter().filter_map(|&i| diff.hunks.get(i)).collect();
    if selected.len() != hunks.len() || selected.is_empty() || diff.change == Change::Unmerged {
        return Err(Error::Unexpected {
            what: format!("no such hunks in {}: {hunks:?}", diff.path().display()),
        });
    }
    let zero_context = selected
        .iter()
        .any(|h| h.lines.iter().all(|l| l.kind != LineKind::Context));
    let mut call = call.args(["--whitespace=nowarn"]);
    if reverse {
        call = call.args(["--reverse"]);
    }
    if zero_context {
        call = call.args(["--unidiff-zero"]);
    }
    let output = git.run(call.input(patch(diff, &selected, reverse)?))?;
    if output.code == 0 {
        return Ok(());
    }
    // `git apply` exits with 1 for a patch it refused and 128 for every other failure.
    if output.code == 1 {
        return Err(Error::PatchDoesNotApply { output: output.said() });
    }
    Err(crate::run::failure(&output))
}

/// A patch of `hunks` alone, their headers renumbered as if the hunks left out were not there. A mode change stays
/// out: it belongs to the file, not to a hunk. A hunk that would start past the largest line number is an error.
pub(crate) fn patch(diff: &FileDiff, hunks: &[&Hunk], reverse: bool) -> Result<Vec<u8>> {
    let path = bytes(diff.path());
    let mut out = b"diff --git ".to_vec();
    out.extend(quoted("a/", path));
    out.push(b' ');
    out.extend(quoted("b/", path));
    out.push(b'\n');
    let old_name = |out: &mut Vec<u8>| name_line(out, b"--- ", "a/", path);
    let new_name = |out: &mut Vec<u8>| name_line(out, b"+++ ", "b/", path);
    match diff.change {
        Change::Added => {
            out.extend(format!("new file mode {:o}\n", diff.new_mode.unwrap_or(0o100_644)).as_bytes());
            out.extend_from_slice(b"--- /dev/null\n");
            new_name(&mut out);
        }
        Change::Deleted => {
            out.extend(format!("deleted file mode {:o}\n", diff.old_mode.unwrap_or(0o100_644)).as_bytes());
            old_name(&mut out);
            out.extend_from_slice(b"+++ /dev/null\n");
        }
        _ => {
            old_name(&mut out);
            new_name(&mut out);
        }
    }
    let mut delta: i64 = 0;
    for hunk in hunks {
        let (old_start, new_start) = renumbered(hunk, delta, reverse)
            .filter(|&(old, new)| numbered_within(hunk, old, new))
            .ok_or_else(|| Error::Unexpected {
                what: format!(
                    "@@ -{},{} +{},{} @@ moved by {delta}",
                    hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
                ),
            })?;
        out.extend(format!("@@ -{old_start},{} +{new_start},{} @@", hunk.old_count, hunk.new_count).as_bytes());
        if !hunk.section.is_empty() {
            out.push(b' ');
            out.extend(hunk.section.as_bytes());
        }
        out.push(b'\n');
        for line in &hunk.lines {
            out.push(match line.kind {
                LineKind::Context => b' ',
                LineKind::Added => b'+',
                LineKind::Removed => b'-',
            });
            out.extend_from_slice(&line.content);
            out.push(b'\n');
            if line.missing_newline {
                out.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
        delta += i64::from(hunk.new_count) - i64::from(hunk.old_count);
    }
    Ok(out)
}

fn name_line(out: &mut Vec<u8>, marker: &[u8], prefix: &str, path: &[u8]) {
    out.extend_from_slice(marker);
    out.extend(quoted(prefix, path));
    // git ends a name with a space in it with a tab, as GNU diff does.
    if path.contains(&b' ') && !out.ends_with(b"\"") {
        out.push(b'\t');
    }
    out.push(b'\n');
}

/// Whether the lines of `hunk`, numbered from `old` and `new`, stay within `u32::MAX`, as the diff reader requires.
fn numbered_within(hunk: &Hunk, old: u32, new: u32) -> bool {
    let on = |side: fn(&LineKind) -> bool| hunk.lines.iter().filter(|line| side(&line.kind)).count() as u64;
    let fits = |start: u32, lines: u64| u64::from(start) + lines.saturating_sub(1) <= u64::from(u32::MAX);
    fits(old, on(|kind| *kind != LineKind::Added)) && fits(new, on(|kind| *kind != LineKind::Removed))
}

/// The starts of `hunk` once only the chosen hunks before it, which shift it by `delta`, are applied. The side the
/// patch is applied to keeps its numbers; the other side moves. `None` when the moved start passes `u32::MAX`.
fn renumbered(hunk: &Hunk, delta: i64, reverse: bool) -> Option<(u32, u32)> {
    // A side without lines names the line before the region, so the region begins one later.
    let begin = |start: u32, count: u32| i64::from(start) + i64::from(count == 0);
    let start = |begin: i64, count: u32| u32::try_from((begin - i64::from(count == 0)).max(0)).ok();
    if reverse {
        let old_begin = begin(hunk.new_start, hunk.new_count) - delta;
        Some((start(old_begin, hunk.old_count)?, hunk.new_start))
    } else {
        let new_begin = begin(hunk.old_start, hunk.old_count) + delta;
        Some((hunk.old_start, start(new_begin, hunk.new_count)?))
    }
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse;

    const TWO_HUNKS: &[u8] = b"diff --git a/f.txt b/f.txt\n\
index 1111111..2222222 100644\n\
--- a/f.txt\n\
+++ b/f.txt\n\
@@ -1,4 +1,5 @@\n\
\x201\n\
+1.5\n\
\x202\n\
\x203\n\
\x204\n\
@@ -10,4 +11,3 @@ fn tail\n\
\x2010\n\
-11\n\
\x2012\n\
\x2013\n";

    #[test]
    fn a_later_hunk_alone_is_renumbered_as_if_the_earlier_one_were_not_there() {
        let diff = &parse(TWO_HUNKS).unwrap()[0];
        let forward = String::from_utf8(patch(diff, &[&diff.hunks[1]], false).unwrap()).unwrap();
        assert!(forward.contains("@@ -10,4 +10,3 @@ fn tail\n"), "{forward}");
        let reverse = String::from_utf8(patch(diff, &[&diff.hunks[1]], true).unwrap()).unwrap();
        assert!(reverse.contains("@@ -11,4 +11,3 @@ fn tail\n"), "{reverse}");
    }

    #[test]
    fn both_hunks_keep_their_numbers() {
        let diff = &parse(TWO_HUNKS).unwrap()[0];
        let both: Vec<&Hunk> = diff.hunks.iter().collect();
        let text = String::from_utf8(patch(diff, &both, false).unwrap()).unwrap();
        assert!(text.contains("@@ -1,4 +1,5 @@\n"));
        assert!(text.contains("@@ -10,4 +11,3 @@"));
        assert!(text.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n"));
    }

    #[test]
    fn regions_without_lines_on_one_side_count_from_the_line_before() {
        let insert = Hunk {
            old_start: 5,
            old_count: 0,
            new_start: 8,
            new_count: 2,
            section: String::new(),
            lines: vec![],
        };
        assert_eq!(renumbered(&insert, 0, false), Some((5, 6)));
        assert_eq!(renumbered(&insert, 3, false), Some((5, 9)));
        let delete = Hunk {
            old_start: 5,
            old_count: 2,
            new_start: 7,
            new_count: 0,
            section: String::new(),
            lines: vec![],
        };
        assert_eq!(renumbered(&delete, 0, false), Some((5, 4)));
        assert_eq!(renumbered(&delete, 0, true), Some((8, 7)));
        let new_file = Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 3,
            section: String::new(),
            lines: vec![],
        };
        assert_eq!(renumbered(&new_file, 0, false), Some((0, 1)));
        assert_eq!(renumbered(&new_file, 0, true), Some((0, 1)));
    }

    #[test]
    fn a_patch_whose_lines_would_be_numbered_past_the_limit_is_refused() {
        let diff =
            parse(b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -4294967294,2 +4294967295,2 @@\n x\n-y\x0b+z\n").unwrap();
        let file = &diff[0];
        assert!(patch(file, &[&file.hunks[0]], false).is_ok());
        assert!(
            patch(file, &[&file.hunks[0]], true).is_err(),
            "the old side would end past u32::MAX"
        );
    }

    #[test]
    fn names_that_need_it_are_quoted_and_spaces_end_with_a_tab() {
        let diff = parse(b"diff --git a/x y b/x y\n--- a/x y\t\n+++ b/x y\t\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        let text = String::from_utf8(patch(&diff[0], &[&diff[0].hunks[0]], false).unwrap()).unwrap();
        assert!(text.contains("--- a/x y\t\n+++ b/x y\t\n"), "{text}");
        let mut quoted_diff = diff[0].clone();
        quoted_diff.new_path = Some(PathBuf::from("new\nline"));
        let text = String::from_utf8(patch(&quoted_diff, &[&quoted_diff.hunks[0]], false).unwrap()).unwrap();
        assert!(text.contains("+++ \"b/new\\nline\"\n"), "{text}");
    }

    #[test]
    fn a_hunk_moved_past_the_largest_line_number_is_an_error() {
        let near_end = u32::MAX - 5;
        let moved = Hunk {
            old_start: near_end,
            old_count: 1,
            new_start: near_end,
            new_count: 1,
            section: String::new(),
            lines: vec![],
        };
        assert_eq!(renumbered(&moved, 5, false), Some((near_end, u32::MAX)));
        assert_eq!(renumbered(&moved, 6, false), None);
        assert_eq!(renumbered(&moved, -6, true), None);
        let text = format!(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -0,0 +1,10 @@\n{}@@ -{near_end},1 +{near_end},1 @@\n-x\n+y\n",
            "+n\n".repeat(10)
        );
        let diff = &parse(text.as_bytes()).unwrap()[0];
        assert!(patch(diff, &[&diff.hunks[1]], false).is_ok());
        let both: Vec<&Hunk> = diff.hunks.iter().collect();
        assert!(matches!(patch(diff, &both, false), Err(Error::Unexpected { .. })));
    }
}
