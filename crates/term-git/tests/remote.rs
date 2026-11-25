//! Push, pull and fetch against a bare repository on disk, with progress, rejection, conflicts and cancelling.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Scratch, read, write};
use term_git::{
    Cancel, CommitOptions, Error, FetchOptions, Operation, Policy, Progress, PullMode, PullOptions, PushFlag,
    PushOptions, Repository,
};

/// A repository with a pushed `main`, and a second clone of the same remote.
fn pair(scratch: &Scratch) -> (Repository, Repository) {
    let mine = scratch.repository("mine", &[("a.txt", &common::numbered(10))]);
    scratch.raw(
        &scratch.dir,
        &["init", "--quiet", "--bare", "--initial-branch=main", "remote.git"],
    );
    scratch.raw(mine.root(), &["remote", "add", "origin", "../remote.git"]);
    let options = PushOptions {
        remote: Some("origin".into()),
        refspec: Some("main".into()),
        set_upstream: true,
    };
    mine.push(&options, &mut |_| {}, &Cancel::new()).unwrap();
    scratch.raw(&scratch.dir, &["clone", "--quiet", "remote.git", "theirs"]);
    scratch.raw(&scratch.path("theirs"), &["config", "user.name", "Other"]);
    scratch.raw(&scratch.path("theirs"), &["config", "user.email", "other@example.org"]);
    let theirs = scratch
        .git()
        .discover(&scratch.path("theirs"), Policy::Trusted)
        .unwrap();
    (mine, theirs)
}

#[test]
fn the_default_branch_is_what_a_clone_recorded_of_the_remote_head() {
    let scratch = Scratch::new("default-branch");
    let (mine, theirs) = pair(&scratch);
    assert_eq!(theirs.default_branch("origin").unwrap().as_deref(), Some("main"));
    assert_eq!(
        mine.default_branch("origin").unwrap(),
        None,
        "a push records no remote HEAD"
    );
    assert_eq!(theirs.default_branch("elsewhere").unwrap(), None);
}

fn commit(repository: &Repository, path: &str, content: &str, message: &str) {
    write(repository.root(), path, content);
    repository.stage(&[path]).unwrap();
    repository.commit(message, CommitOptions::default()).unwrap();
}

fn quiet() -> impl FnMut(&Progress) {
    |_| {}
}

#[test]
fn a_first_push_sets_the_upstream_and_reports_progress() {
    let scratch = Scratch::new("push");
    let repository = scratch.repository("r", &[("big.bin", &"x".repeat(300_000))]);
    scratch.raw(&scratch.dir, &["init", "--quiet", "--bare", "remote.git"]);
    scratch.raw(repository.root(), &["remote", "add", "origin", "../remote.git"]);
    let mut reports = Vec::new();
    let options = PushOptions {
        remote: Some("origin".into()),
        refspec: Some("HEAD".into()),
        set_upstream: true,
    };
    let outcome = repository
        .push(&options, &mut |p| reports.push(p.clone()), &Cancel::new())
        .unwrap();
    assert_eq!(outcome.refs.len(), 1);
    assert_eq!(outcome.refs[0].flag, PushFlag::New);
    assert_eq!(outcome.refs[0].to, "refs/heads/main");
    assert!(
        reports
            .iter()
            .any(|p| p.phase == "Writing objects" && p.percent == Some(100)),
        "{reports:?}"
    );
    let status = repository.status(term_git::StatusOptions::default()).unwrap();
    assert_eq!(status.branch.upstream.as_deref(), Some("origin/main"));
    let again = repository
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    assert_eq!(again.refs[0].flag, PushFlag::UpToDate);
}

#[test]
fn a_push_behind_the_remote_is_rejected_until_forced() {
    let scratch = Scratch::new("rejected");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "b.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "c.txt", "mine\n", "mine");
    match mine.push(&PushOptions::default(), &mut quiet(), &Cancel::new()) {
        Err(Error::PushRejected { refs, .. }) => assert_eq!(refs[0].flag, PushFlag::Rejected),
        other => panic!("{other:?}"),
    }
    mine.fetch(&FetchOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    let outcome = mine
        .destructive()
        .force_push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    assert_eq!(outcome.refs[0].flag, PushFlag::Forced);
}

#[test]
fn a_failing_pre_push_hook_stops_the_push() {
    let scratch = Scratch::new("pre-push");
    let (mine, _) = pair(&scratch);
    let hook = mine.root().join(".git/hooks/pre-push");
    std::fs::write(&hook, "#!/bin/sh\necho 'tests failed' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    commit(&mine, "c.txt", "c\n", "c");
    match mine.push(&PushOptions::default(), &mut quiet(), &Cancel::new()) {
        Err(Error::HookFailed { hook, output }) => {
            assert_eq!(hook, "pre-push");
            assert!(output.contains("tests failed"));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn pull_fast_forwards_and_refuses_diverged_branches_unless_rebasing() {
    let scratch = Scratch::new("pull");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "b.txt", "one\n", "theirs one");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    mine.pull(&PullOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    assert_eq!(read(mine.root(), "b.txt"), "one\n");

    commit(&theirs, "b.txt", "two\n", "theirs two");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "c.txt", "mine\n", "mine");
    assert!(matches!(
        mine.pull(&PullOptions::default(), &mut quiet(), &Cancel::new()),
        Err(Error::NotFastForward { .. })
    ));
    let rebase = PullOptions {
        mode: PullMode::Rebase,
        ..PullOptions::default()
    };
    mine.pull(&rebase, &mut quiet(), &Cancel::new()).unwrap();
    let log = mine.log(3).unwrap();
    assert_eq!(log[0].subject, "mine");
    assert_eq!(log[1].subject, "theirs two");
}

#[test]
fn a_rebase_that_conflicts_names_the_files() {
    let scratch = Scratch::new("pull-conflict");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "a.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "a.txt", "mine\n", "mine");
    let rebase = PullOptions {
        mode: PullMode::Rebase,
        ..PullOptions::default()
    };
    match mine.pull(&rebase, &mut quiet(), &Cancel::new()) {
        Err(Error::Conflicts { paths, .. }) => assert_eq!(paths, [PathBuf::from("a.txt")]),
        other => panic!("{other:?}"),
    }
}

#[test]
fn pulling_over_local_changes_names_them() {
    let scratch = Scratch::new("pull-dirty");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "a.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    write(mine.root(), "a.txt", "uncommitted\n");
    match mine.pull(&PullOptions::default(), &mut quiet(), &Cancel::new()) {
        Err(Error::LocalChanges { paths, .. }) => assert_eq!(paths, [PathBuf::from("a.txt")]),
        other => panic!("{other:?}"),
    }
    assert_eq!(read(mine.root(), "a.txt"), "uncommitted\n");
}

#[test]
fn fetch_brings_new_remote_branches_and_prunes_gone_ones() {
    let scratch = Scratch::new("fetch");
    let (mine, theirs) = pair(&scratch);
    theirs.create_branch("topic", None).unwrap();
    let topic = PushOptions {
        remote: Some("origin".into()),
        refspec: Some("topic".into()),
        set_upstream: false,
    };
    theirs.push(&topic, &mut quiet(), &Cancel::new()).unwrap();
    mine.fetch(&FetchOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    assert!(mine.branches().unwrap().iter().any(|b| b.name == "origin/topic"));
    let delete = PushOptions {
        remote: Some("origin".into()),
        refspec: Some(":topic".into()),
        set_upstream: false,
    };
    let outcome = theirs.push(&delete, &mut quiet(), &Cancel::new()).unwrap();
    assert_eq!(outcome.refs[0].flag, PushFlag::Deleted);
    let prune = FetchOptions {
        prune: true,
        all: true,
        ..FetchOptions::default()
    };
    mine.fetch(&prune, &mut quiet(), &Cancel::new()).unwrap();
    assert!(mine.branches().unwrap().iter().all(|b| b.name != "origin/topic"));
}

#[test]
fn an_unreachable_remote_is_a_network_error() {
    let scratch = Scratch::new("network");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    scratch.raw(
        repository.root(),
        &["remote", "add", "origin", "https://127.0.0.1:9/nothing.git"],
    );
    let result = repository.fetch(&FetchOptions::default(), &mut quiet(), &Cancel::new());
    assert!(matches!(result, Err(Error::Network { .. })), "{result:?}");
}

#[test]
fn a_hanging_fetch_stops_when_cancelled() {
    let scratch = Scratch::new("cancel");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let root = repository.root();
    // The ssh command never answers, as a server that hangs would.
    scratch.raw(root, &["config", "core.sshCommand", "sh -c 'sleep 30'"]);
    scratch.raw(root, &["remote", "add", "origin", "ssh://example.invalid/repo.git"]);
    let cancel = Cancel::new();
    let trigger = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        trigger.cancel();
    });
    let started = Instant::now();
    let result = repository.fetch(&FetchOptions::default(), &mut quiet(), &cancel);
    assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn remote_names_that_look_like_options_are_refused() {
    let scratch = Scratch::new("options");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let options = FetchOptions {
        remote: Some("--upload-pack=touch x".into()),
        ..FetchOptions::default()
    };
    assert!(matches!(
        repository.fetch(&options, &mut quiet(), &Cancel::new()),
        Err(Error::InvalidName { .. })
    ));
}

#[test]
fn a_merging_pull_joins_diverged_branches_in_a_merge_commit() {
    let scratch = Scratch::new("pull-merge");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "b.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "c.txt", "mine\n", "mine");
    let merge = PullOptions {
        mode: PullMode::Merge,
        ..PullOptions::default()
    };
    mine.pull(&merge, &mut quiet(), &Cancel::new()).unwrap();
    let log = mine.log(1).unwrap();
    assert_eq!(log[0].parents.len(), 2);
    assert_eq!(read(mine.root(), "b.txt"), "theirs\n");
    assert_eq!(mine.operation().unwrap(), None);
}

#[test]
fn a_conflicting_merge_waits_until_its_files_are_resolved_and_then_commits() {
    let scratch = Scratch::new("merge-continue");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "a.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "a.txt", "mine\n", "mine");
    let merge = PullOptions {
        mode: PullMode::Merge,
        ..PullOptions::default()
    };
    assert!(matches!(
        mine.pull(&merge, &mut quiet(), &Cancel::new()),
        Err(Error::Conflicts { .. })
    ));
    assert_eq!(mine.operation().unwrap(), Some(Operation::Merge));
    match mine.continue_operation() {
        Err(Error::Conflicts { paths, .. }) => assert_eq!(paths, [PathBuf::from("a.txt")]),
        other => panic!("{other:?}"),
    }
    write(mine.root(), "a.txt", "both\n");
    mine.stage(&["a.txt"]).unwrap();
    mine.continue_operation().unwrap();
    assert_eq!(mine.operation().unwrap(), None);
    assert_eq!(mine.log(1).unwrap()[0].parents.len(), 2);
}

#[test]
fn an_aborted_rebase_leaves_the_branch_as_it_was() {
    let scratch = Scratch::new("rebase-abort");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "a.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "a.txt", "mine\n", "mine");
    let before = mine.head().unwrap();
    let rebase = PullOptions {
        mode: PullMode::Rebase,
        ..PullOptions::default()
    };
    assert!(mine.pull(&rebase, &mut quiet(), &Cancel::new()).is_err());
    assert_eq!(mine.operation().unwrap(), Some(Operation::Rebase));
    mine.destructive().abort_operation().unwrap();
    assert_eq!(mine.operation().unwrap(), None);
    assert_eq!(mine.head().unwrap(), before);
    assert_eq!(read(mine.root(), "a.txt"), "mine\n");
}

#[test]
fn a_resolved_rebase_continues_to_its_end() {
    let scratch = Scratch::new("rebase-continue");
    let (mine, theirs) = pair(&scratch);
    commit(&theirs, "a.txt", "theirs\n", "theirs");
    theirs
        .push(&PushOptions::default(), &mut quiet(), &Cancel::new())
        .unwrap();
    commit(&mine, "a.txt", "mine\n", "mine");
    let rebase = PullOptions {
        mode: PullMode::Rebase,
        ..PullOptions::default()
    };
    assert!(mine.pull(&rebase, &mut quiet(), &Cancel::new()).is_err());
    write(mine.root(), "a.txt", "both\n");
    mine.stage(&["a.txt"]).unwrap();
    mine.continue_operation().unwrap();
    assert_eq!(mine.operation().unwrap(), None);
    let log = mine.log(2).unwrap();
    assert_eq!(log[0].subject, "mine");
    assert_eq!(log[1].subject, "theirs");
}

#[test]
fn a_branch_without_upstream_says_so_for_push_and_pull() {
    let scratch = Scratch::new("no-upstream");
    let lonely = scratch.repository("lonely", &[("a.txt", "a\n")]);
    assert!(matches!(
        lonely.push(&PushOptions::default(), &mut quiet(), &Cancel::new()),
        Err(Error::NoUpstream { .. })
    ));
    let (mine, _) = pair(&scratch);
    mine.switch_to_new_branch("topic", None).unwrap();
    assert!(matches!(
        mine.pull(&PullOptions::default(), &mut quiet(), &Cancel::new()),
        Err(Error::NoUpstream { .. })
    ));
}
