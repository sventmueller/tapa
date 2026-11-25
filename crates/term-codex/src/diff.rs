//! A file change as hunks: Codex sends a unified diff for a changed file and the whole content for an added or
//! deleted one.

use term_thread::Hunk;

/// The numbers of a `@@ -a,b +c,d @@` header; a missing count is one line.
fn header(line: &str) -> Option<(u64, u64, u64, u64)> {
    let inner = line.strip_prefix("@@ -")?.split(" @@").next()?;
    let (old, new) = inner.split_once(" +")?;
    let range = |text: &str| -> Option<(u64, u64)> {
        match text.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((text.parse().ok()?, 1)),
        }
    };
    let (old_start, old_lines) = range(old)?;
    let (new_start, new_lines) = range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

/// The hunks of the unified diff `diff`; file headers and `\ No newline` markers are left out.
pub(crate) fn unified(diff: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in diff.lines() {
        if let Some((old_start, old_lines, new_start, new_lines)) = header(line) {
            hunks.push(Hunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        if line.starts_with([' ', '-', '+']) {
            hunk.lines.push(line.to_string());
        } else if line.is_empty() {
            hunk.lines.push(" ".into());
        }
    }
    hunks
}

/// `content` as one hunk of lines that were added, or with `removed`, deleted.
pub(crate) fn whole(content: &str, removed: bool) -> Vec<Hunk> {
    let lines: Vec<String> = content
        .lines()
        .map(|l| format!("{}{l}", if removed { '-' } else { '+' }))
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let count = lines.len() as u64;
    let (old_lines, new_lines) = if removed { (count, 0) } else { (0, count) };
    vec![Hunk {
        old_start: u64::from(removed),
        old_lines,
        new_start: u64::from(!removed),
        new_lines,
        lines,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unified_diff_reads_as_its_hunks() {
        let diff = "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+2\n\\ No newline at end of file\n@@ -9 +9,2 @@\n x\n+y\n";
        let hunks = unified(diff);
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[0].old_start, hunks[0].old_lines, hunks[0].new_lines), (1, 2, 2));
        assert_eq!(hunks[0].lines, [" one", "-two", "+2"]);
        assert_eq!((hunks[1].old_lines, hunks[1].new_start, hunks[1].new_lines), (1, 9, 2));
        assert!(unified("not a diff").is_empty());
    }

    #[test]
    fn a_new_file_is_all_added_lines() {
        let hunks = whole("a\nb\n", false);
        assert_eq!(hunks[0].lines, ["+a", "+b"]);
        assert_eq!((hunks[0].old_lines, hunks[0].new_start, hunks[0].new_lines), (0, 1, 2));
        assert_eq!(whole("a", true)[0].lines, ["-a"]);
        assert!(whole("", false).is_empty());
    }
}
