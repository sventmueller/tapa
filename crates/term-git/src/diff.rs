//! Diffs of single files, read into hunks with line numbers on both sides.

use std::borrow::Cow;
use std::ffi::OsString;
use std::iter::Peekable;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::path::{all_relative, from_bytes, relative, unquoted};
use crate::repo::Repository;
use crate::status::{EntryKind, StatusEntry};

/// Which two states of a file a diff compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    /// The index against the working tree: what is not staged.
    Unstaged,
    /// HEAD against the index: what is staged.
    Staged,
    /// HEAD against the working tree: both.
    AgainstHead,
}

/// What happened to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// It is new.
    Added,
    /// It was deleted.
    Deleted,
    /// Its content or mode changed.
    Modified,
    /// It was renamed, perhaps with changes.
    Renamed {
        /// The similarity in percent.
        similarity: u8,
    },
    /// It was copied, perhaps with changes.
    Copied {
        /// The similarity in percent.
        similarity: u8,
    },
    /// It has a merge conflict; its diff has no hunks.
    Unmerged,
}

/// The diff of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// The path before, relative to the root; `None` for a new file.
    pub old_path: Option<PathBuf>,
    /// The path after; `None` for a deleted file.
    pub new_path: Option<PathBuf>,
    /// What happened to the file.
    pub change: Change,
    /// The mode before, such as `0o100644`, when git names it.
    pub old_mode: Option<u32>,
    /// The mode after.
    pub new_mode: Option<u32>,
    /// Whether git treats the file as binary, in which case there are no hunks.
    pub binary: bool,
    /// The changed regions.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path after, or before for a deleted file.
    pub fn path(&self) -> &Path {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or(Path::new(""))
    }

    fn empty(old_path: Option<PathBuf>, new_path: Option<PathBuf>) -> FileDiff {
        FileDiff {
            old_path,
            new_path,
            change: Change::Modified,
            old_mode: None,
            new_mode: None,
            binary: false,
            hunks: Vec::new(),
        }
    }
}

/// One changed region of a file with its context.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Hunk {
    /// The first line before; for a region without old lines, the line after which it starts.
    pub old_start: u32,
    /// How many lines it has before.
    pub old_count: u32,
    /// The first line after, counted the same way.
    pub new_start: u32,
    /// How many lines it has after.
    pub new_count: u32,
    /// The text after the second `@@`, often the enclosing function.
    pub section: String,
    pub lines: Vec<Line>,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Whether it stayed, was added or removed.
    pub kind: LineKind,
    /// Its number before; `None` for an added line.
    pub old_number: Option<u32>,
    /// Its number after; `None` for a removed line.
    pub new_number: Option<u32>,
    /// Its bytes without the line end; a carriage return of a CRLF file stays.
    pub content: Vec<u8>,
    /// Whether it is the file's last line and has no line end.
    pub missing_newline: bool,
}

impl Line {
    /// The content as text, with invalid UTF-8 replaced.
    pub fn text(&self) -> Cow<'_, str> {
        String::from_utf8_lossy(&self.content)
    }
}

/// The kind of a line in a hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Unchanged context.
    Context,
    /// Added.
    Added,
    /// Removed.
    Removed,
}

/// The arguments every diff gets so that its output parses the same whatever the user configured.
pub(crate) fn diff_args() -> Vec<OsString> {
    [
        "-c",
        "core.quotepath=false",
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--unified=3",
    ]
    .iter()
    .map(OsString::from)
    .collect()
}

/// `paths` as pathspec arguments that match only themselves.
pub(crate) fn literal_args(paths: &[PathBuf]) -> Vec<OsString> {
    paths
        .iter()
        .map(|p| {
            let mut bytes = b":(literal)".to_vec();
            bytes.extend_from_slice(crate::path::bytes(p));
            OsString::from_vec(bytes)
        })
        .collect()
}

impl Repository {
    /// The diffs of `paths`, or of every changed file when `paths` is empty. A staged rename shows as one only when
    /// both its paths are given; [`Repository::diff_entry`] takes care of that.
    ///
    /// ```no_run
    /// use std::path::Path;
    ///
    /// use term_git::{DiffSource, Git, LineKind, Policy};
    /// use term_process::Supervisor;
    ///
    /// let git = Git::new(Supervisor::global(), std::env::vars().collect());
    /// let repository = git.discover(Path::new("."), Policy::Trusted)?;
    /// for file in repository.diff(DiffSource::Unstaged, &["README.md"])? {
    ///     assert_eq!(file.path(), Path::new("README.md"));
    ///     for line in file.hunks.iter().flat_map(|h| &h.lines).filter(|l| l.kind == LineKind::Added) {
    ///         assert!(line.old_number.is_none());
    ///     }
    /// }
    /// # Ok::<(), term_git::Error>(())
    /// ```
    pub fn diff<P: AsRef<Path>>(&self, source: DiffSource, paths: &[P]) -> Result<Vec<FileDiff>> {
        let paths = all_relative(self.root(), paths)?;
        let mut args = diff_args();
        match source {
            DiffSource::Unstaged => {}
            DiffSource::Staged => args.extend(["--cached", "-M"].map(OsString::from)),
            DiffSource::AgainstHead => {
                args.push("-M".into());
                let base = match self.head()? {
                    Some(head) => head.to_string(),
                    None => self.empty_tree()?,
                };
                args.push(base.into());
            }
        }
        args.push("--".into());
        args.extend(literal_args(&paths));
        let output = self.git().ok(self.call(args))?;
        parse(&output.stdout)
    }

    /// The diff of one status entry; an untracked file shows as new.
    pub fn diff_entry(&self, entry: &StatusEntry, source: DiffSource) -> Result<Option<FileDiff>> {
        let mut paths = vec![entry.path.clone()];
        match (&entry.kind, source) {
            (EntryKind::Untracked, _) => return self.diff_untracked(&entry.path).map(Some),
            (EntryKind::Ignored, _) => return Ok(None),
            (
                EntryKind::Renamed { from, .. } | EntryKind::Copied { from, .. },
                DiffSource::Staged | DiffSource::AgainstHead,
            ) => {
                paths.push(from.clone());
            }
            _ => {}
        }
        let diffs = self.diff(source, &paths)?;
        Ok(diffs.into_iter().find(|d| d.path() == entry.path))
    }

    /// An untracked file as a diff from nothing.
    pub fn diff_untracked(&self, path: &Path) -> Result<FileDiff> {
        let path = relative(self.root(), path)?;
        let mut args = diff_args();
        args.extend(["--no-index", "--", "/dev/null"].map(OsString::from));
        args.push(path.clone().into_os_string());
        let output = self.git().run(self.call(args))?;
        if output.code > 1 {
            return Err(crate::run::failure(&output));
        }
        let mut diff = parse(&output.stdout)?
            .into_iter()
            .next()
            .unwrap_or_else(|| FileDiff::empty(None, None));
        diff.change = Change::Added;
        diff.old_path = None;
        diff.new_path = Some(path);
        Ok(diff)
    }
}

/// The file diffs in the output of `git diff`.
pub(crate) fn parse(output: &[u8]) -> Result<Vec<FileDiff>> {
    let mut files = Vec::new();
    let mut current: Option<FileDiff> = None;
    let mut combined = false;
    let output = output.strip_suffix(b"\n").unwrap_or(output);
    let mut lines = output.split(|&b| b == b'\n').peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix(b"diff --git ") {
            files.extend(current.take());
            let (old, new) = header_paths(rest);
            current = Some(FileDiff::empty(
                old.map(|p| from_bytes(&p)),
                new.map(|p| from_bytes(&p)),
            ));
            combined = false;
        } else if let Some(rest) = line
            .strip_prefix(b"diff --cc ")
            .or_else(|| line.strip_prefix(b"diff --combined "))
        {
            files.extend(current.take());
            let path = Some(from_bytes(&unquoted(rest)));
            let mut diff = FileDiff::empty(path.clone(), path);
            diff.change = Change::Unmerged;
            current = Some(diff);
            combined = true;
        } else if let Some(rest) = line.strip_prefix(b"* Unmerged path ") {
            files.extend(current.take());
            let path = Some(from_bytes(&unquoted(rest)));
            let mut diff = FileDiff::empty(path.clone(), path);
            diff.change = Change::Unmerged;
            files.push(diff);
        } else if combined {
            continue;
        } else if let Some(file) = current.as_mut() {
            if line.starts_with(b"@@ ") {
                let unexpected = || Error::Unexpected {
                    what: String::from_utf8_lossy(line).into_owned(),
                };
                let mut hunk = hunk_header(line).ok_or_else(unexpected)?;
                hunk_body(&mut hunk, &mut lines).ok_or_else(unexpected)?;
                file.hunks.push(hunk);
            } else {
                header_line(file, line);
            }
        }
    }
    files.extend(current.take());
    Ok(files)
}

/// The two paths of `a/<old> b/<new>`, without their prefixes.
fn header_paths(rest: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let strip = |name: Vec<u8>, prefix: &[u8]| name.strip_prefix(prefix).map(<[u8]>::to_vec);
    if rest.first() == Some(&b'"') {
        let end = closing_quote(rest);
        let old = unquoted(&rest[..=end]);
        let new = unquoted(rest.get(end + 2..).unwrap_or_default());
        return (strip(old, b"a/"), strip(new, b"b/"));
    }
    if rest.last() == Some(&b'"')
        && let Some(start) = rest.windows(2).position(|w| w == b" \"")
    {
        return (
            strip(rest[..start].to_vec(), b"a/"),
            strip(unquoted(&rest[start + 1..]), b"b/"),
        );
    }
    // Without a rename both names are the same, which settles where a name with spaces ends.
    if rest.len() > 5 && (rest.len() - 5).is_multiple_of(2) {
        let n = (rest.len() - 5) / 2;
        let (old, new) = (&rest[2..2 + n], &rest[5 + n..]);
        if rest.starts_with(b"a/") && &rest[2 + n..5 + n] == b" b/" && old == new {
            return (Some(old.to_vec()), Some(new.to_vec()));
        }
    }
    match rest.windows(3).position(|w| w == b" b/") {
        Some(split) => (strip(rest[..split].to_vec(), b"a/"), Some(rest[split + 3..].to_vec())),
        None => (None, None),
    }
}

/// The index of the quote that closes the one at the start of `text`.
fn closing_quote(text: &[u8]) -> usize {
    let mut i = 1;
    while i < text.len() {
        match text[i] {
            b'\\' => i += 2,
            b'"' => return i,
            _ => i += 1,
        }
    }
    text.len() - 1
}

/// A path in a `---`, `+++`, `rename` or `copy` line, without prefix; `None` for `/dev/null`.
fn line_path(text: &[u8], prefix: &[u8]) -> Option<PathBuf> {
    let text = if text.first() == Some(&b'"') {
        unquoted(text)
    } else {
        text.strip_suffix(b"\t").unwrap_or(text).to_vec()
    };
    if text == b"/dev/null" {
        return None;
    }
    Some(from_bytes(text.strip_prefix(prefix).unwrap_or(&text)))
}

fn mode(text: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(text).ok()?.trim(), 8).ok()
}

fn similarity(text: &[u8]) -> u8 {
    std::str::from_utf8(text)
        .ok()
        .and_then(|t| t.trim_end_matches('%').parse().ok())
        .unwrap_or(0)
}

fn header_line(file: &mut FileDiff, line: &[u8]) {
    if let Some(rest) = line.strip_prefix(b"old mode ") {
        file.old_mode = mode(rest);
    } else if let Some(rest) = line.strip_prefix(b"new mode ") {
        file.new_mode = mode(rest);
    } else if let Some(rest) = line.strip_prefix(b"deleted file mode ") {
        file.change = Change::Deleted;
        file.old_mode = mode(rest);
        file.new_path = None;
    } else if let Some(rest) = line.strip_prefix(b"new file mode ") {
        file.change = Change::Added;
        file.new_mode = mode(rest);
        file.old_path = None;
    } else if let Some(rest) = line.strip_prefix(b"similarity index ") {
        file.change = Change::Renamed {
            similarity: similarity(rest),
        };
    } else if let Some(rest) = line.strip_prefix(b"rename from ") {
        file.old_path = line_path(rest, b"");
        if let Change::Copied { similarity } = file.change {
            file.change = Change::Renamed { similarity };
        }
    } else if let Some(rest) = line.strip_prefix(b"rename to ") {
        file.new_path = line_path(rest, b"");
    } else if let Some(rest) = line.strip_prefix(b"copy from ") {
        let similarity = match file.change {
            Change::Renamed { similarity } => similarity,
            _ => 0,
        };
        file.change = Change::Copied { similarity };
        file.old_path = line_path(rest, b"");
    } else if let Some(rest) = line.strip_prefix(b"copy to ") {
        file.new_path = line_path(rest, b"");
    } else if let Some(rest) = line.strip_prefix(b"index ") {
        let mode = rest.split(|&b| b == b' ').nth(1).and_then(mode);
        file.old_mode = file.old_mode.or(mode);
        file.new_mode = file.new_mode.or(mode);
    } else if let Some(rest) = line.strip_prefix(b"--- ") {
        file.old_path = line_path(rest, b"a/");
    } else if let Some(rest) = line.strip_prefix(b"+++ ") {
        file.new_path = line_path(rest, b"b/");
    } else if line.starts_with(b"Binary files ") || line == b"GIT binary patch" {
        file.binary = true;
    }
}

/// The hunk that starts with `line`, still without its lines.
fn hunk_header(line: &[u8]) -> Option<Hunk> {
    let text = std::str::from_utf8(line).ok()?;
    let rest = text.strip_prefix("@@ -")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(u32, u32)> {
        match r.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let ((old_start, old_count), (new_start, new_count)) = (range(old)?, range(new)?);
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.strip_prefix(' ').unwrap_or(section).to_string(),
        lines: Vec::new(),
    })
}

/// Reads as many lines into `hunk` as its header counts, and the marks of a missing final line end; `None` when a line
/// would be numbered past `u32::MAX`.
fn hunk_body<'a>(hunk: &mut Hunk, lines: &mut Peekable<impl Iterator<Item = &'a [u8]>>) -> Option<()> {
    let (mut old_left, mut new_left) = (hunk.old_count, hunk.new_count);
    let (mut old, mut new) = (Some(hunk.old_start), Some(hunk.new_start));
    while let Some(&line) = lines.peek() {
        let (kind, content) = match line.first() {
            Some(b'\\') => {
                if let Some(last) = hunk.lines.last_mut() {
                    last.missing_newline = true;
                }
                lines.next();
                continue;
            }
            _ if old_left == 0 && new_left == 0 => break,
            Some(b' ') if old_left > 0 && new_left > 0 => (LineKind::Context, &line[1..]),
            None if old_left > 0 && new_left > 0 => (LineKind::Context, line),
            Some(b'-') if old_left > 0 => (LineKind::Removed, &line[1..]),
            Some(b'+') if new_left > 0 => (LineKind::Added, &line[1..]),
            _ => break,
        };
        let (old_number, new_number) = match kind {
            LineKind::Context => (Some(old?), Some(new?)),
            LineKind::Removed => (Some(old?), None),
            LineKind::Added => (None, Some(new?)),
        };
        if let Some(number) = old_number {
            old = number.checked_add(1);
            old_left -= 1;
        }
        if let Some(number) = new_number {
            new = number.checked_add(1);
            new_left -= 1;
        }
        hunk.lines.push(Line {
            kind,
            old_number,
            new_number,
            content: content.to_vec(),
            missing_newline: false,
        });
        lines.next();
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_modified_file_is_read_with_line_numbers_on_both_sides() {
        let diff = b"diff --git a/src/x.rs b/src/x.rs\n\
index 814f4a4..55b5b73 100644\n\
--- a/src/x.rs\n\
+++ b/src/x.rs\n\
@@ -1,3 +1,4 @@ fn main()\n\
\x20one\n\
-two\n\
+TWO\n\
+2.5\n\
\x20three\n";
        let files = parse(diff).unwrap();
        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path(), Path::new("src/x.rs"));
        assert_eq!(file.change, Change::Modified);
        assert_eq!((file.old_mode, file.new_mode), (Some(0o100_644), Some(0o100_644)));
        let hunk = &file.hunks[0];
        assert_eq!(
            (hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count),
            (1, 3, 1, 4)
        );
        assert_eq!(hunk.section, "fn main()");
        let kinds: Vec<_> = hunk
            .lines
            .iter()
            .map(|l| (l.kind, l.old_number, l.new_number))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (LineKind::Context, Some(1), Some(1)),
                (LineKind::Removed, Some(2), None),
                (LineKind::Added, None, Some(2)),
                (LineKind::Added, None, Some(3)),
                (LineKind::Context, Some(3), Some(4)),
            ]
        );
        assert_eq!(hunk.lines[2].text(), "TWO");
    }

    #[test]
    fn lines_that_look_like_headers_inside_a_hunk_are_content() {
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n--- old\n+++ new\n-x\n+y\n";
        let files = parse(diff);
        assert!(files.is_ok());
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n--- not a header\n+++ nor this\n";
        let file = &parse(diff).unwrap()[0];
        assert_eq!(file.hunks[0].lines.len(), 2);
        assert_eq!(file.hunks[0].lines[0].content, b"-- not a header");
        assert_eq!(file.hunks[0].lines[0].kind, LineKind::Removed);
    }

    #[test]
    fn a_missing_line_end_is_marked_on_its_line() {
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file\n";
        let lines = &parse(diff).unwrap()[0].hunks[0].lines;
        assert!(lines[0].missing_newline);
        assert!(lines[1].missing_newline);
    }

    #[test]
    fn new_deleted_renamed_and_binary_files_are_told_apart() {
        let diff = b"diff --git a/new.txt b/new.txt\n\
new file mode 100755\n\
index 0000000..7898192\n\
--- /dev/null\n\
+++ b/new.txt\n\
@@ -0,0 +1 @@\n\
+a\n\
diff --git a/gone.txt b/gone.txt\n\
deleted file mode 100644\n\
index 7898192..0000000\n\
--- a/gone.txt\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-a\n\
diff --git a/sp ace.txt b/moved sp.txt\n\
similarity index 90%\n\
rename from sp ace.txt\n\
rename to moved sp.txt\n\
diff --git a/img.png b/img.png\n\
index 1111111..2222222 100644\n\
Binary files a/img.png and b/img.png differ\n";
        let files = parse(diff).unwrap();
        assert_eq!(files.len(), 4);
        assert_eq!(files[0].change, Change::Added);
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[0].new_mode, Some(0o100_755));
        assert_eq!(files[0].hunks[0].lines[0].new_number, Some(1));
        assert_eq!(files[1].change, Change::Deleted);
        assert_eq!(files[1].new_path, None);
        assert_eq!(files[1].path(), Path::new("gone.txt"));
        assert_eq!(files[2].change, Change::Renamed { similarity: 90 });
        assert_eq!(files[2].old_path.as_deref(), Some(Path::new("sp ace.txt")));
        assert_eq!(files[2].new_path.as_deref(), Some(Path::new("moved sp.txt")));
        assert!(files[3].binary);
        assert!(files[3].hunks.is_empty());
    }

    #[test]
    fn quoted_names_and_names_with_spaces_and_tabs_are_read() {
        let diff = b"diff --git \"a/new\\nline.txt\" \"b/new\\nline.txt\"\n\
--- \"a/new\\nline.txt\"\n\
+++ \"b/new\\nline.txt\"\n\
@@ -1 +1,2 @@\n\
\x20x\n\
+y\n\
diff --git a/x y.txt b/x y.txt\n\
--- a/x y.txt\t\n\
+++ b/x y.txt\t\n\
@@ -1 +1 @@\n\
-a\n\
+b\n\
diff --git a/b c.bin b/b c.bin\n\
Binary files a/b c.bin and b/b c.bin differ\n";
        let files = parse(diff).unwrap();
        assert_eq!(files[0].path(), Path::new("new\nline.txt"));
        assert_eq!(files[1].path(), Path::new("x y.txt"));
        assert_eq!(files[1].old_path.as_deref(), Some(Path::new("x y.txt")));
        assert_eq!(files[2].path(), Path::new("b c.bin"));
    }

    #[test]
    fn conflicted_files_have_no_hunks() {
        let diff = b"diff --cc c.txt\nindex 1,2..0\n--- a/c.txt\n+++ b/c.txt\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< HEAD\n* Unmerged path d.txt\n";
        let files = parse(diff).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.change == Change::Unmerged && f.hunks.is_empty()));
        assert_eq!(files[1].path(), Path::new("d.txt"));
    }

    #[test]
    fn an_empty_context_line_counts_as_context() {
        let diff = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n x\n\n-y\n+z\n";
        let hunk = &parse(diff).unwrap()[0].hunks[0];
        assert_eq!(hunk.lines.len(), 4);
        assert_eq!(hunk.lines[1].kind, LineKind::Context);
        assert!(hunk.lines[1].content.is_empty());
    }

    #[test]
    fn a_hunk_numbered_past_the_largest_line_number_is_an_error() {
        let past = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -4294967295,2 +1,2 @@\n x\n y\n";
        assert!(matches!(parse(past), Err(Error::Unexpected { .. })));
        let added = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,0 +4294967295,2 @@\n+x\n+y\n";
        assert!(matches!(parse(added), Err(Error::Unexpected { .. })));
        let last = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -4294967295 +4294967295 @@\n-x\n+y\n";
        let hunk = &parse(last).unwrap()[0].hunks[0];
        assert_eq!(
            hunk.lines[0].old_number,
            Some(u32::MAX),
            "the largest number itself is a line"
        );
        assert_eq!(hunk.lines[1].new_number, Some(u32::MAX));
    }
}
