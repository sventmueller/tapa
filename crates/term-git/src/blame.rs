//! Who last changed each line of a file, from `git blame --porcelain --incremental`, and the whole message of a
//! commit.

use std::collections::HashMap;
use std::path::Path;

use crate::error::Result;
use crate::id::CommitId;
use crate::remote::word;
use crate::repo::Repository;

/// A commit that last changed some lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameCommit {
    /// Its name.
    pub hash: CommitId,
    /// The author's name.
    pub author: String,
    /// The author's email address, without its angle brackets.
    pub author_email: String,
    /// When it was authored, in seconds since 1970.
    pub author_time: i64,
    /// The first line of its message.
    pub summary: String,
}

/// The last change of every line of a file at one commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blame {
    /// The commits, each once.
    pub(crate) commits: Vec<BlameCommit>,
    /// For each line of the file, counted from 0, the index of its commit in `commits`; `None` for a line the output
    /// left out.
    pub lines: Vec<Option<usize>>,
}

impl Blame {
    /// The commit that last changed `line`, counted from 0.
    pub fn commit_of(&self, line: usize) -> Option<&BlameCommit> {
        self.commits.get((*self.lines.get(line)?)?)
    }
}

impl Repository {
    /// Who last changed each line of `path`, relative to the root, as of `revision`; lines a later change moved keep
    /// their first author, since git follows moves within the file.
    pub fn blame(&self, revision: &str, path: &Path) -> Result<Blame> {
        let mut args: Vec<std::ffi::OsString> = [
            "blame",
            "--porcelain",
            "--incremental",
            "--no-textconv",
            word(revision)?,
            "--",
        ]
        .map(std::ffi::OsString::from)
        .to_vec();
        // Blame takes a plain path, not a pathspec; after `--` git reads it as one whatever it starts with.
        args.push(path.as_os_str().to_os_string());
        let output = self.git().ok(self.call(args))?;
        Ok(parse_incremental(&output.stdout))
    }

    /// The whole message of `commit`, its subject first.
    pub fn commit_message(&self, commit: &CommitId) -> Result<String> {
        let output = self.git().ok(self.call([
            "-c",
            "log.showSignature=false",
            "show",
            "--no-patch",
            "--no-color",
            "--format=%B",
            commit.as_str(),
            "--",
        ]))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
    }
}

/// More lines than any file blame is asked for has; a header past it is broken output, not a reason to allocate.
const MAX_LINES: usize = 10_000_000;

/// The lines of `git blame --porcelain --incremental`: per group of lines a header `<hash> <source line> <final line>
/// <count>`, the commit's details the first time it appears, and a `filename` line that ends the group.
pub(crate) fn parse_incremental(output: &[u8]) -> Blame {
    let text = String::from_utf8_lossy(output);
    let mut blame = Blame::default();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut current: Option<usize> = None;
    for line in text.lines() {
        if let Some((hash, final_line, count)) = group_header(line) {
            let at = *index.entry(hash.to_string()).or_insert_with(|| {
                blame.commits.push(BlameCommit {
                    hash: CommitId::from_git(hash),
                    author: String::new(),
                    author_email: String::new(),
                    author_time: 0,
                    summary: String::new(),
                });
                blame.commits.len() - 1
            });
            current = Some(at);
            let end = final_line.saturating_add(count);
            if end > MAX_LINES {
                current = None;
                continue;
            }
            if blame.lines.len() < end {
                blame.lines.resize(end, None);
            }
            for slot in &mut blame.lines[final_line..end] {
                *slot = Some(at);
            }
            continue;
        }
        let Some(commit) = current.and_then(|at| blame.commits.get_mut(at)) else {
            continue;
        };
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "author" => value.clone_into(&mut commit.author),
            "author-mail" => {
                value
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .clone_into(&mut commit.author_email);
            }
            "author-time" => commit.author_time = value.parse().unwrap_or(0),
            "summary" => value.clone_into(&mut commit.summary),
            "filename" => current = None,
            _ => {}
        }
    }
    blame
}

/// The hash, the first final line counted from 0 and the number of lines of a group's header.
fn group_header(line: &str) -> Option<(&str, usize, usize)> {
    let mut fields = line.split(' ');
    let hash = fields.next()?;
    if !(hash.len() == 40 || hash.len() == 64) || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let _source: usize = fields.next()?.parse().ok()?;
    let final_line: usize = fields.next()?.parse().ok()?;
    let count: usize = fields.next()?.parse().ok()?;
    if fields.next().is_some() || final_line == 0 {
        return None;
    }
    Some((hash, final_line - 1, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    fn output() -> String {
        format!(
            "{B} 3 3 1\nauthor Bea\nauthor-mail <bea@example.com>\nauthor-time 1700000300\nauthor-tz +0100\n\
             committer Bea\ncommitter-mail <bea@example.com>\ncommitter-time 1700000300\ncommitter-tz +0100\n\
             summary Change the third line\nprevious {A} file.txt\nfilename file.txt\n\
             {A} 1 1 2\nauthor Ada\nauthor-mail <ada@example.com>\nauthor-time 1700000000\nauthor-tz +0000\n\
             committer Ada\ncommitter-mail <ada@example.com>\ncommitter-time 1700000000\ncommitter-tz +0000\n\
             summary First lines\nboundary\nfilename file.txt\n\
             {A} 4 4 1\nfilename file.txt\n"
        )
    }

    #[test]
    fn each_line_gets_its_commit_whose_details_come_once() {
        let blame = parse_incremental(output().as_bytes());
        assert_eq!(blame.commits.len(), 2);
        assert_eq!(blame.lines, vec![Some(1), Some(1), Some(0), Some(1)]);
        let third = blame.commit_of(2).unwrap();
        assert_eq!(third.hash.as_str(), B);
        assert_eq!(third.author, "Bea");
        assert_eq!(third.author_email, "bea@example.com");
        assert_eq!(third.author_time, 1_700_000_300);
        assert_eq!(third.summary, "Change the third line");
        let last = blame.commit_of(3).unwrap();
        assert_eq!((last.author.as_str(), last.summary.as_str()), ("Ada", "First lines"));
        assert!(blame.commit_of(4).is_none());
    }

    #[test]
    fn a_summary_that_looks_like_a_header_stays_a_summary() {
        let tricky = format!("{A} 1 1 1\nauthor Ada\nsummary {B} 1 1 1\nfilename f\n");
        let blame = parse_incremental(tricky.as_bytes());
        assert_eq!(blame.commits.len(), 1);
        assert_eq!(blame.commits[0].summary, format!("{B} 1 1 1"));
    }

    #[test]
    fn broken_output_gives_what_it_can_and_never_panics() {
        for broken in [
            "",
            "garbage\n",
            &format!("{A} 0 0 1\n"),
            &format!("{A} 1 1\n"),
            "\u{fffd}\n",
        ] {
            let blame = parse_incremental(broken.as_bytes());
            assert!(blame.lines.iter().all(Option::is_none), "{broken:?}");
        }
        let huge = format!("{A} 1 2 3\nfilename f\n");
        assert_eq!(
            parse_incremental(huge.as_bytes()).lines,
            vec![None, Some(0), Some(0), Some(0)]
        );
    }
}
