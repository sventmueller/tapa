//! Blame and commit messages on temporary repositories.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::path::Path;

use common::{Scratch, write};

#[test]
fn blame_names_the_commit_of_each_line_at_a_revision() {
    let scratch = Scratch::new("blame");
    let repository = scratch.repository("r", &[("src/a.txt", "one\ntwo\nthree\n")]);
    let root = repository.root().to_path_buf();
    write(&root, "src/a.txt", "one\nTWO\nthree\nfour\n");
    scratch.raw(
        &root,
        &["commit", "--quiet", "-am", "Change two\n\nAnd add a fourth line."],
    );
    write(&root, "src/a.txt", "uncommitted\n");
    let blame = repository.blame("HEAD", Path::new("src/a.txt")).unwrap();
    assert_eq!(blame.lines.len(), 4, "the committed file, not the working tree");
    let summaries: Vec<&str> = (0..4)
        .map(|line| blame.commit_of(line).unwrap().summary.as_str())
        .collect();
    assert_eq!(summaries, ["first", "Change two", "first", "Change two"]);
    let second = blame.commit_of(1).unwrap();
    assert_eq!(
        (second.author.as_str(), second.author_email.as_str()),
        ("Test", "test@example.org")
    );
    assert!(second.author_time > 1_600_000_000);
    assert_eq!(
        repository.commit_message(&second.hash).unwrap(),
        "Change two\n\nAnd add a fourth line."
    );
}

#[test]
fn blame_of_a_file_the_revision_lacks_fails() {
    let scratch = Scratch::new("blame-missing");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    assert!(repository.blame("HEAD", Path::new("missing.txt")).is_err());
    assert!(
        repository.blame("-x", Path::new("a.txt")).is_err(),
        "no option where a revision goes"
    );
}
