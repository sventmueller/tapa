//! The stash: parking changes, listing and reading entries, applying them back, and the state an apply that meets
//! conflicts leaves until it is finished or abandoned.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::path::{Path, PathBuf};

use common::{Scratch, read, write};
use term_git::{Applied, Change, Error, LineKind, Repository, StashOptions, StatusOptions, Unstash};

fn changed(repository: &Repository) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = repository
        .status(StatusOptions::default())
        .unwrap()
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    paths.sort();
    paths
}

fn message(text: &str) -> StashOptions {
    StashOptions {
        message: text.into(),
        ..StashOptions::default()
    }
}

#[test]
fn stashing_parks_changes_lists_them_newest_first_and_says_when_there_was_nothing() {
    let scratch = Scratch::new("stash-list");
    let repository = scratch.repository("r", &[("a.txt", "a\n"), ("b.txt", "b\n")]);
    assert!(!repository.stash(&StashOptions::default()).unwrap());
    assert!(repository.stashes().unwrap().is_empty());

    write(repository.root(), "a.txt", "a changed\n");
    assert!(repository.stash(&message("first")).unwrap());
    write(repository.root(), "b.txt", "b changed\n");
    write(repository.root(), "new.txt", "new\n");
    let options = StashOptions {
        message: "second: with new".into(),
        untracked: true,
        staged_only: false,
    };
    assert!(repository.stash(&options).unwrap());
    assert!(changed(&repository).is_empty(), "{:?}", changed(&repository));

    let stashes = repository.stashes().unwrap();
    let listed: Vec<(usize, &str)> = stashes.iter().map(|s| (s.index, s.message.as_str())).collect();
    assert_eq!(listed, [(0, "On main: second: with new"), (1, "On main: first")]);
    assert!(stashes.iter().all(|s| s.hash.as_str().len() >= 40 && s.time > 0));

    let files: Vec<(PathBuf, Change)> = repository
        .stash_changes(&stashes[0].hash)
        .unwrap()
        .into_iter()
        .map(|f| (f.path, f.change))
        .collect();
    assert_eq!(
        files,
        [
            (PathBuf::from("b.txt"), Change::Modified),
            (PathBuf::from("new.txt"), Change::Added)
        ]
    );
    let new = repository
        .diff_stash(&stashes[0].hash, Path::new("new.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(new.change, Change::Added);
    assert_eq!(new.hunks[0].lines[0].kind, LineKind::Added);
    let b = repository
        .diff_stash(&stashes[0].hash, Path::new("b.txt"))
        .unwrap()
        .unwrap();
    assert_eq!(b.hunks[0].lines.len(), 2);
    assert!(
        repository
            .diff_stash(&stashes[0].hash, Path::new("a.txt"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn only_what_is_staged_can_be_stashed() {
    let scratch = Scratch::new("stash-staged");
    let repository = scratch.repository("r", &[("a.txt", "a\n"), ("b.txt", "b\n")]);
    write(repository.root(), "a.txt", "a staged\n");
    write(repository.root(), "b.txt", "b not staged\n");
    repository.stage(&["a.txt"]).unwrap();
    let options = StashOptions {
        staged_only: true,
        ..StashOptions::default()
    };
    assert!(repository.stash(&options).unwrap());
    assert_eq!(read(repository.root(), "a.txt"), "a\n");
    assert_eq!(read(repository.root(), "b.txt"), "b not staged\n");
    assert_eq!(changed(&repository), [PathBuf::from("b.txt")]);
}

#[test]
fn applying_keeps_the_entry_popping_removes_it() {
    let scratch = Scratch::new("stash-apply");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write(repository.root(), "a.txt", "a changed\n");
    write(repository.root(), "new.txt", "new\n");
    let options = StashOptions {
        untracked: true,
        ..StashOptions::default()
    };
    repository.stash(&options).unwrap();
    let stash = repository.stashes().unwrap().remove(0);
    assert_eq!(repository.apply_stash(&stash, Unstash::Apply).unwrap(), Applied::Clean);
    assert_eq!(read(repository.root(), "a.txt"), "a changed\n");
    assert_eq!(read(repository.root(), "new.txt"), "new\n");
    assert_eq!(repository.stashes().unwrap().len(), 1);
    assert_eq!(repository.stash_apply().unwrap(), None);

    scratch.raw(repository.root(), &["checkout", "--quiet", "--", "a.txt"]);
    std::fs::remove_file(repository.root().join("new.txt")).unwrap();
    assert_eq!(repository.apply_stash(&stash, Unstash::Pop).unwrap(), Applied::Clean);
    assert_eq!(read(repository.root(), "a.txt"), "a changed\n");
    assert!(repository.stashes().unwrap().is_empty());
}

#[test]
fn an_entry_that_moved_is_neither_applied_nor_dropped() {
    let scratch = Scratch::new("stash-moved");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write(repository.root(), "a.txt", "one\n");
    repository.stash(&message("one")).unwrap();
    let old = repository.stashes().unwrap().remove(0);
    write(repository.root(), "a.txt", "two\n");
    repository.stash(&message("two")).unwrap();
    assert!(matches!(
        repository.apply_stash(&old, Unstash::Apply),
        Err(Error::StashMoved)
    ));
    assert!(matches!(
        repository.destructive().drop_stash(&old),
        Err(Error::StashMoved)
    ));
    let now = repository.stashes().unwrap();
    repository.destructive().drop_stash(&now[1]).unwrap();
    let left: Vec<String> = repository.stashes().unwrap().into_iter().map(|s| s.message).collect();
    assert_eq!(left, ["On main: two"]);
    repository.destructive().clear_stashes().unwrap();
    assert!(repository.stashes().unwrap().is_empty());
}

/// A repository whose stash changes line 2 of `a.txt` while HEAD changed it otherwise since; `b.txt` has a change of
/// its own that is not staged and `staged.txt` one that is, and `brought.txt` is untracked in the stash.
fn conflicting(scratch: &Scratch) -> Repository {
    let repository = scratch.repository("r", &[("a.txt", "1\n2\n3\n"), ("b.txt", "b\n"), ("staged.txt", "s\n")]);
    write(repository.root(), "a.txt", "1\nstashed\n3\n");
    write(repository.root(), "brought.txt", "brought\n");
    let options = StashOptions {
        message: "work".into(),
        untracked: true,
        staged_only: false,
    };
    repository.stash(&options).unwrap();
    write(repository.root(), "a.txt", "1\ncommitted\n3\n");
    scratch.raw(repository.root(), &["commit", "--quiet", "-am", "other"]);
    write(repository.root(), "b.txt", "b local\n");
    write(repository.root(), "staged.txt", "s local\n");
    repository.stage(&["staged.txt"]).unwrap();
    repository
}

#[test]
fn a_conflict_on_apply_waits_and_abandoning_it_puts_back_the_state_before() {
    let scratch = Scratch::new("stash-conflict");
    let repository = conflicting(&scratch);
    let before = scratch.raw(repository.root(), &["status", "--porcelain=v1"]);
    let stash = repository.stashes().unwrap().remove(0);
    assert_eq!(
        repository.apply_stash(&stash, Unstash::Apply).unwrap(),
        Applied::Conflicts(vec![PathBuf::from("a.txt")])
    );
    assert!(read(repository.root(), "a.txt").contains("<<<<<<<"));
    assert!(repository.root().join("brought.txt").exists());
    let waiting = repository.stash_apply().unwrap().expect("the apply waits");
    assert_eq!((waiting.stash.as_str(), waiting.pop), (stash.hash.as_str(), false));
    assert_eq!(repository.operation().unwrap(), None);

    repository.destructive().abort_stash_apply().unwrap();
    assert_eq!(repository.stash_apply().unwrap(), None);
    assert_eq!(read(repository.root(), "a.txt"), "1\ncommitted\n3\n");
    assert_eq!(read(repository.root(), "b.txt"), "b local\n");
    assert_eq!(read(repository.root(), "staged.txt"), "s local\n");
    assert!(!repository.root().join("brought.txt").exists());
    assert_eq!(scratch.raw(repository.root(), &["status", "--porcelain=v1"]), before);
    assert_eq!(repository.stashes().unwrap().len(), 1);
}

#[test]
fn a_pop_with_conflicts_keeps_its_entry_and_finishing_marks_the_conflicts_resolved() {
    let scratch = Scratch::new("stash-pop-conflict");
    let repository = conflicting(&scratch);
    let stash = repository.stashes().unwrap().remove(0);
    assert!(matches!(
        repository.apply_stash(&stash, Unstash::Pop).unwrap(),
        Applied::Conflicts(_)
    ));
    assert_eq!(repository.stashes().unwrap(), std::slice::from_ref(&stash));
    assert!(repository.stash_apply().unwrap().is_some_and(|s| s.pop));

    write(repository.root(), "a.txt", "1\nboth\n3\n");
    repository.finish_stash_apply().unwrap();
    assert_eq!(repository.stash_apply().unwrap(), None);
    let unmerged = scratch.raw(repository.root(), &["diff", "--name-only", "--diff-filter=U"]);
    assert!(unmerged.is_empty(), "{unmerged}");
    assert_eq!(read(repository.root(), "a.txt"), "1\nboth\n3\n");
    assert_eq!(repository.stashes().unwrap().len(), 1);
}

#[test]
fn local_changes_in_the_way_leave_everything_as_it_was() {
    let scratch = Scratch::new("stash-in-the-way");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write(repository.root(), "a.txt", "stashed\n");
    repository.stash(&StashOptions::default()).unwrap();
    write(repository.root(), "a.txt", "local\n");
    let stash = repository.stashes().unwrap().remove(0);
    assert!(matches!(
        repository.apply_stash(&stash, Unstash::Pop),
        Err(Error::LocalChanges { .. })
    ));
    assert_eq!(read(repository.root(), "a.txt"), "local\n");
    assert_eq!(repository.stash_apply().unwrap(), None);
    assert_eq!(repository.stashes().unwrap().len(), 1);
}

#[test]
fn a_waiting_apply_is_forgotten_once_head_moves_on() {
    let scratch = Scratch::new("stash-stale");
    let repository = conflicting(&scratch);
    let stash = repository.stashes().unwrap().remove(0);
    repository.apply_stash(&stash, Unstash::Apply).unwrap();
    write(repository.root(), "a.txt", "1\nresolved\n3\n");
    scratch.raw(repository.root(), &["add", "--all"]);
    scratch.raw(
        repository.root(),
        &["commit", "--quiet", "-m", "resolved in a terminal"],
    );
    assert_eq!(repository.stash_apply().unwrap(), None);
}
