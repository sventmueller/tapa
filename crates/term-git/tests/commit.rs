//! Commits with hooks that pass and fail.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use common::{Scratch, write};
use term_git::{CommitOptions, Error};

fn hook(root: &Path, name: &str, script: &str) {
    let path = root.join(".git/hooks").join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_commit_returns_its_hash_and_takes_the_message_as_given() {
    let scratch = Scratch::new("commit");
    let repository = scratch.repository("r", &[]);
    write(repository.root(), "a.txt", "a\n");
    repository.stage(&["a.txt"]).unwrap();
    let message = "First line with ümlauts\n\nBody with # hash and \"quotes\"\n";
    let hash = repository.commit(message, CommitOptions::default()).unwrap();
    assert_eq!(repository.head().unwrap(), Some(hash));
    let stored = scratch.raw(repository.root(), &["log", "-1", "--format=%B"]);
    assert_eq!(stored.trim_end(), message.trim_end());
}

#[test]
fn empty_messages_and_empty_commits_are_refused_before_hooks_run() {
    let scratch = Scratch::new("refused");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let marker = scratch.path("hook-ran");
    hook(
        repository.root(),
        "pre-commit",
        &format!("touch '{}'", marker.display()),
    );
    assert!(matches!(
        repository.commit("  \n", CommitOptions::default()),
        Err(Error::EmptyMessage)
    ));
    assert!(matches!(
        repository.commit("nothing", CommitOptions::default()),
        Err(Error::NothingToCommit)
    ));
    assert!(!marker.exists());
}

#[test]
fn a_failing_pre_commit_hook_is_named_with_its_output() {
    let scratch = Scratch::new("pre-commit");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    hook(
        repository.root(),
        "pre-commit",
        "echo 'lint: a.txt is ugly'\necho 'to stderr' >&2\nexit 3",
    );
    write(repository.root(), "a.txt", "b\n");
    repository.stage(&["a.txt"]).unwrap();
    let before = repository.head().unwrap();
    match repository.commit("change", CommitOptions::default()) {
        Err(Error::HookFailed { hook, output }) => {
            assert_eq!(hook, "pre-commit");
            assert!(
                output.contains("lint: a.txt is ugly") && output.contains("to stderr"),
                "{output}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(repository.head().unwrap(), before);
}

#[test]
fn a_failing_commit_msg_hook_is_told_from_a_failing_pre_commit_hook() {
    let scratch = Scratch::new("commit-msg");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    hook(repository.root(), "pre-commit", "git diff --cached --quiet; exit 0");
    hook(
        repository.root(),
        "commit-msg",
        "grep -q '^JIRA-' \"$1\" || { echo 'needs a ticket' >&2; exit 1; }",
    );
    write(repository.root(), "a.txt", "b\n");
    repository.stage(&["a.txt"]).unwrap();
    match repository.commit("no ticket", CommitOptions::default()) {
        Err(Error::HookFailed { hook, output }) => {
            assert_eq!(hook, "commit-msg");
            assert!(output.contains("needs a ticket"));
        }
        other => panic!("{other:?}"),
    }
    repository
        .commit("JIRA-1 with ticket", CommitOptions::default())
        .unwrap();
}

#[test]
fn amending_keeps_the_message_when_none_is_given_and_signoff_adds_a_trailer() {
    let scratch = Scratch::new("amend");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write(repository.root(), "a.txt", "b\n");
    repository.stage(&["a.txt"]).unwrap();
    let first = repository.commit("Original", CommitOptions::default()).unwrap();
    write(repository.root(), "b.txt", "b\n");
    repository.stage(&["b.txt"]).unwrap();
    let amended = repository
        .commit(
            "",
            CommitOptions {
                amend: true,
                signoff: false,
            },
        )
        .unwrap();
    assert_ne!(first, amended);
    let log = repository.log(5).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "Original");
    repository
        .commit(
            "Reworded",
            CommitOptions {
                amend: true,
                signoff: true,
            },
        )
        .unwrap();
    let message = scratch.raw(repository.root(), &["log", "-1", "--format=%B"]);
    assert!(message.starts_with("Reworded"));
    assert!(message.contains("Signed-off-by: Test <test@example.org>"));
}
