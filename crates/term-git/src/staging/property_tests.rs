//! A patch of some hunks of a diff, applied as `git apply` applies it with both its starts checked, makes exactly
//! the changes of those hunks, forwards for staging and backwards for unstaging; checked over generated files.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

use super::patch;
use crate::diff::{Hunk, LineKind, parse};

fn config() -> Config {
    Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x5_7a6e),
        ..Config::default()
    }
}

/// One changed region of a file and the unchanged lines before it.
#[derive(Debug, Clone)]
struct Region {
    before: Vec<String>,
    removed: Vec<String>,
    added: Vec<String>,
}

fn line() -> impl Strategy<Value = String> {
    (0u8..6).prop_map(|n| format!("line {n}"))
}

fn region() -> impl Strategy<Value = Region> {
    let lines = |most| proptest::collection::vec(line(), 0..most);
    (lines(4), lines(4), lines(4)).prop_map(|(before, removed, mut added)| {
        if removed.is_empty() && added.is_empty() {
            added.push("added".into());
        }
        Region { before, removed, added }
    })
}

/// Regions with unchanged lines between them, so that each is a hunk of its own.
fn file() -> impl Strategy<Value = (Vec<Region>, Vec<String>)> {
    (
        proptest::collection::vec(region(), 1..7),
        proptest::collection::vec(line(), 0..3),
    )
        .prop_map(|(mut regions, tail)| {
            for region in regions.iter_mut().skip(1) {
                if region.before.is_empty() {
                    region.before.push("between".into());
                }
            }
            (regions, tail)
        })
}

/// The file's lines with the regions `changed` picks in their new form, the others in their old one.
fn lines(regions: &[Region], tail: &[String], changed: impl Fn(usize) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for (n, region) in regions.iter().enumerate() {
        out.extend(region.before.iter().cloned());
        out.extend(if changed(n) { &region.added } else { &region.removed }.iter().cloned());
    }
    out.extend(tail.iter().cloned());
    out
}

/// The regions as `git diff --unified=0` prints them.
fn diff(regions: &[Region]) -> String {
    let mut out = String::from("diff --git a/f.txt b/f.txt\nindex 1111111..2222222 100644\n--- a/f.txt\n+++ b/f.txt\n");
    let (mut old, mut new) = (0, 0);
    for region in regions {
        old += region.before.len();
        new += region.before.len();
        let (removed, added) = (region.removed.len(), region.added.len());
        let old_start = old + usize::from(removed > 0);
        let new_start = new + usize::from(added > 0);
        out.push_str(&format!("@@ -{old_start},{removed} +{new_start},{added} @@\n"));
        for line in &region.removed {
            out.push_str(&format!("-{line}\n"));
        }
        for line in &region.added {
            out.push_str(&format!("+{line}\n"));
        }
        old += removed;
        new += added;
    }
    out
}

/// `target` with `hunks` applied, backwards when `reverse`; each hunk must find its lines where its start on the
/// side it reads says, and its start on the side it writes must be where the result has got to.
fn apply(target: &[String], hunks: &[Hunk], reverse: bool) -> Result<Vec<String>, String> {
    let index = |start: u32, count: u32| {
        usize::try_from(start)
            .ok()
            .and_then(|start| start.checked_sub(usize::from(count > 0)))
            .ok_or(format!("start {start} with {count} lines"))
    };
    let mut out = Vec::new();
    let mut read = 0;
    for hunk in hunks {
        let (from, to) = if reverse {
            (
                index(hunk.new_start, hunk.new_count)?,
                index(hunk.old_start, hunk.old_count)?,
            )
        } else {
            (
                index(hunk.old_start, hunk.old_count)?,
                index(hunk.new_start, hunk.new_count)?,
            )
        };
        if from < read || from > target.len() {
            return Err(format!("{hunk:?} reads at {from} after {read}"));
        }
        out.extend(target[read..from].iter().cloned());
        read = from;
        if out.len() != to {
            return Err(format!("{hunk:?} writes at {to}, the result is at {}", out.len()));
        }
        for line in &hunk.lines {
            let content = String::from_utf8(line.content.clone()).map_err(|e| e.to_string())?;
            let (reads, writes) = match (line.kind, reverse) {
                (LineKind::Context, _) => (true, true),
                (LineKind::Removed, false) | (LineKind::Added, true) => (true, false),
                (LineKind::Added, false) | (LineKind::Removed, true) => (false, true),
            };
            if reads {
                if target.get(read) != Some(&content) {
                    return Err(format!("{hunk:?} expects {content:?} at {read}"));
                }
                read += 1;
            }
            if writes {
                out.push(content);
            }
        }
    }
    out.extend(target[read..].iter().cloned());
    Ok(out)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn a_patch_of_some_hunks_makes_their_changes_and_no_others(
        (regions, tail) in file(),
        mask in any::<u32>(),
    ) {
        let text = diff(&regions);
        let files = parse(text.as_bytes()).expect("the diff reads");
        let file = &files[0];
        prop_assert_eq!(file.hunks.len(), regions.len());
        let mut chosen: Vec<bool> = (0..regions.len()).map(|n| (mask >> n) & 1 == 1).collect();
        if !chosen.contains(&true) {
            chosen[usize::try_from(mask).unwrap_or(0) % regions.len()] = true;
        }
        let hunks: Vec<&Hunk> = file.hunks.iter().zip(&chosen).filter(|(_, c)| **c).map(|(h, _)| h).collect();
        let old = lines(&regions, &tail, |_| false);
        let new = lines(&regions, &tail, |_| true);

        let staged = parse(&patch(file, &hunks, false).expect("the patch is written")).expect("the patch reads");
        let staged = apply(&old, &staged[0].hunks, false);
        prop_assert_eq!(staged, Ok(lines(&regions, &tail, |n| chosen[n])));

        let unstaged = parse(&patch(file, &hunks, true).expect("the patch is written")).expect("the patch reads");
        let unstaged = apply(&new, &unstaged[0].hunks, true);
        prop_assert_eq!(unstaged, Ok(lines(&regions, &tail, |n| !chosen[n])));
    }
}
