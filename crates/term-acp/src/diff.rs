//! Hunks between two versions of a file, for the diffs agents report as old and new text.

use term_thread::Hunk;

/// Lines of unchanged context around each change.
const CONTEXT: usize = 3;

/// Above this many line pairs the texts are shown as one replacement instead of being compared line by line.
const MAX_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Removed,
    Added,
}

/// The line operations turning `old` into `new`, by longest common subsequence.
fn ops(old: &[&str], new: &[&str]) -> Vec<Op> {
    if old.len().saturating_mul(new.len()) > MAX_CELLS {
        return [vec![Op::Removed; old.len()], vec![Op::Added; new.len()]].concat();
    }
    let width = new.len() + 1;
    let mut table = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i * width + j] = if old[i] == new[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::with_capacity(old.len() + new.len()));
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            out.push(Op::Same);
            i += 1;
            j += 1;
        } else if i < old.len() && (j == new.len() || table[(i + 1) * width + j] >= table[i * width + j + 1]) {
            out.push(Op::Removed);
            i += 1;
        } else {
            out.push(Op::Added);
            j += 1;
        }
    }
    out
}

/// The hunks that turn `old` into `new`, each with up to three lines of context.
pub(crate) fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let ops = ops(&old, &new);
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| **op != Op::Same)
        .map(|(n, _)| n)
        .collect();
    let mut hunks = Vec::new();
    let mut at = 0;
    while at < changed.len() {
        let first = changed[at];
        let mut last = first;
        while at + 1 < changed.len() && changed[at + 1] <= last + 2 * CONTEXT + 1 {
            at += 1;
            last = changed[at];
        }
        at += 1;
        let start = first.saturating_sub(CONTEXT);
        let end = (last + CONTEXT + 1).min(ops.len());
        hunks.push(hunk(&ops, &old, &new, start, end));
    }
    hunks
}

/// The hunk covering operations `start..end`.
fn hunk(ops: &[Op], old: &[&str], new: &[&str], start: usize, end: usize) -> Hunk {
    let before = &ops[..start];
    let old_start = before.iter().filter(|op| **op != Op::Added).count();
    let new_start = before.iter().filter(|op| **op != Op::Removed).count();
    let (mut i, mut j) = (old_start, new_start);
    let mut lines = Vec::new();
    for op in &ops[start..end] {
        match op {
            Op::Same => {
                lines.push(format!(" {}", old[i]));
                i += 1;
                j += 1;
            }
            Op::Removed => {
                lines.push(format!("-{}", old[i]));
                i += 1;
            }
            Op::Added => {
                lines.push(format!("+{}", new[j]));
                j += 1;
            }
        }
    }
    let old_lines = (i - old_start) as u64;
    let new_lines = (j - new_start) as u64;
    Hunk {
        old_start: if old_lines == 0 {
            old_start as u64
        } else {
            old_start as u64 + 1
        },
        old_lines,
        new_start: if new_lines == 0 {
            new_start as u64
        } else {
            new_start as u64 + 1
        },
        new_lines,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_line_becomes_one_hunk_with_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nd\nE\nf\ng\nh\n";
        let hunks = hunks(old, new);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!((h.old_start, h.old_lines, h.new_start, h.new_lines), (2, 7, 2, 7));
        assert_eq!(h.lines, [" b", " c", " d", "-e", "+E", " f", " g", " h"]);
    }

    #[test]
    fn a_new_file_is_all_additions_and_distant_changes_stay_apart() {
        let created = hunks("", "one\ntwo\n");
        assert_eq!(
            (created[0].old_start, created[0].old_lines, created[0].new_start),
            (0, 0, 1)
        );
        assert_eq!(created[0].lines, ["+one", "+two"]);
        let line = |n: u32| match n {
            2 => "two\n".to_string(),
            29 => "twenty-nine\n".to_string(),
            n => format!("{n}\n"),
        };
        let old: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let new: String = (1..=30).map(line).collect();
        assert_eq!(hunks(&old, &new).len(), 2);
    }
}
