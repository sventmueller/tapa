//! Cherry-picks and reverts that stop at conflicts, a rebase that stops again at its next commit, a hook that refuses
//! the merge commit, and `git am`, which the app leaves alone.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use common::{Scratch, write};
use term_git::{CommitOptions, Error, Operation, Repository};

fn commit(repository: &Repository, path: &str, content: &str, message: &str) -> String {
    write(repository.root(), path, content);
    repository.stage(&[path]).unwrap();
    repository.commit(message, CommitOptions::default()).unwrap();
    repository.head().unwrap().unwrap().to_string()
}

/// A repository whose `topic` branch changed `a.txt` one way and whose `main` changed it another.
fn diverged(scratch: &Scratch, name: &str) -> (Repository, String) {
    let repo = scratch.repository(name, &[("a.txt", "base\n")]);
    scratch.raw(repo.root(), &["switch", "--quiet", "-c", "topic"]);
    let topic = commit(&repo, "a.txt", "topic\n", "topic");
    scratch.raw(repo.root(), &["switch", "--quiet", "main"]);
    commit(&repo, "a.txt", "main\n", "main");
    (repo, topic)
}

fn conflicted(result: term_git::Result<()>) -> Vec<PathBuf> {
    match result {
        Err(Error::Conflicts { paths, .. }) => paths,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_conflicting_cherry_pick_waits_and_then_commits_the_picked_change() {
    let scratch = Scratch::new("cherry-pick");
    let (repo, topic) = diverged(&scratch, "repo");
    assert!(!scratch.try_raw(repo.root(), &["cherry-pick", &topic]));
    assert_eq!(repo.operation().unwrap(), Some(Operation::CherryPick));
    assert_eq!(conflicted(repo.continue_operation()), [PathBuf::from("a.txt")]);
    write(repo.root(), "a.txt", "both\n");
    repo.stage(&["a.txt"]).unwrap();
    repo.continue_operation().unwrap();
    assert_eq!(repo.operation().unwrap(), None);
    assert_eq!(repo.log(1).unwrap()[0].subject, "topic");
}

#[test]
fn a_conflicting_revert_is_recognised_and_abandoned() {
    let scratch = Scratch::new("revert");
    let repo = scratch.repository("repo", &[("a.txt", "base\n")]);
    let changed = commit(&repo, "a.txt", "one\n", "one");
    commit(&repo, "a.txt", "two\n", "two");
    let before = repo.head().unwrap();
    assert!(!scratch.try_raw(repo.root(), &["revert", "--no-edit", &changed]));
    assert_eq!(repo.operation().unwrap(), Some(Operation::Revert));
    repo.destructive().abort_operation().unwrap();
    assert_eq!(repo.operation().unwrap(), None);
    assert_eq!(repo.head().unwrap(), before);
    assert_eq!(common::read(repo.root(), "a.txt"), "two\n");
}

#[test]
fn a_rebase_that_meets_the_next_conflict_stops_there_again() {
    let scratch = Scratch::new("rebase-twice");
    let repo = scratch.repository("repo", &[("a.txt", "base\n")]);
    scratch.raw(repo.root(), &["switch", "--quiet", "-c", "topic"]);
    commit(&repo, "a.txt", "topic one\n", "one");
    commit(&repo, "a.txt", "topic two\n", "two");
    scratch.raw(repo.root(), &["switch", "--quiet", "main"]);
    commit(&repo, "a.txt", "main\n", "main");
    scratch.raw(repo.root(), &["switch", "--quiet", "topic"]);
    assert!(!scratch.try_raw(repo.root(), &["rebase", "main"]));
    write(repo.root(), "a.txt", "resolved one\n");
    repo.stage(&["a.txt"]).unwrap();
    assert_eq!(conflicted(repo.continue_operation()), [PathBuf::from("a.txt")]);
    assert_eq!(repo.operation().unwrap(), Some(Operation::Rebase));
    repo.destructive().abort_operation().unwrap();
    assert_eq!(repo.operation().unwrap(), None);
}

#[test]
fn a_hook_that_refuses_the_merge_commit_is_named() {
    let scratch = Scratch::new("merge-hook");
    let (repo, _) = diverged(&scratch, "repo");
    assert!(!scratch.try_raw(repo.root(), &["merge", "--no-edit", "topic"]));
    write(repo.root(), "a.txt", "both\n");
    repo.stage(&["a.txt"]).unwrap();
    let hook = repo.git_dir().join("hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'merges need a review' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    match repo.continue_operation() {
        Err(Error::HookFailed { hook, output }) => {
            assert_eq!(hook, "pre-commit");
            assert!(output.contains("merges need a review"), "{output}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(repo.operation().unwrap(), Some(Operation::Merge));
}

#[test]
fn nothing_waits_when_no_operation_stopped_or_git_am_did() {
    let scratch = Scratch::new("am");
    let repo = scratch.repository("repo", &[("a.txt", "base\n")]);
    assert_eq!(repo.operation().unwrap(), None);
    repo.continue_operation().unwrap();
    repo.destructive().abort_operation().unwrap();
    let apply = repo.git_dir().join("rebase-apply");
    std::fs::create_dir_all(&apply).unwrap();
    std::fs::write(apply.join("applying"), "").unwrap();
    assert_eq!(repo.operation().unwrap(), None);
    std::fs::remove_file(apply.join("applying")).unwrap();
    assert_eq!(repo.operation().unwrap(), Some(Operation::Rebase));
}
