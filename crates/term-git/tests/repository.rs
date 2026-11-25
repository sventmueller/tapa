//! Finding repositories, reading their status, branches, history, remotes and worktrees, and the destructive
//! operations, on temporary repositories.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::path::{Path, PathBuf};

use common::{Scratch, read, write};
use term_git::{Change, Conflict, EntryKind, Error, FileState, Policy, StatusOptions, Untracked};

#[test]
fn a_folder_inside_a_repository_finds_its_root_and_git_folders() {
    let scratch = Scratch::new("discover");
    let repository = scratch.repository("r", &[("src/main.rs", "fn main() {}\n")]);
    let found = scratch
        .git()
        .discover(&repository.root().join("src/main.rs"), Policy::Trusted)
        .unwrap();
    assert_eq!(found.root(), scratch.path("r"));
    assert_eq!(found.git_dir(), scratch.path("r/.git"));
    assert_eq!(found.common_dir(), scratch.path("r/.git"));
    assert!(!found.is_linked_worktree());
    assert!(found.head().unwrap().is_some());
}

#[test]
fn a_folder_outside_any_repository_and_a_bare_repository_are_told_apart() {
    let scratch = Scratch::new("not-a-repo");
    std::fs::create_dir_all(scratch.path("plain")).unwrap();
    let git = scratch.git();
    assert!(matches!(
        git.discover(&scratch.path("plain"), Policy::Trusted),
        Err(Error::NotARepository { .. })
    ));
    scratch.raw(&scratch.dir, &["init", "--quiet", "--bare", "bare.git"]);
    assert!(matches!(
        git.discover(&scratch.path("bare.git"), Policy::Trusted),
        Err(Error::BareRepository { .. })
    ));
    assert!(
        matches!(
            git.discover(&scratch.path("bare.git"), Policy::Untrusted),
            Err(Error::NotARepository { .. })
        ),
        "an untrusted folder's bare repository is only taken when named explicitly"
    );
}

#[test]
fn init_makes_a_plain_folder_a_repository_without_commits() {
    let scratch = Scratch::new("init");
    std::fs::create_dir_all(scratch.path("plain")).unwrap();
    let git = scratch.git();
    let repository = git.init(&scratch.path("plain"), Policy::Trusted).unwrap();
    assert_eq!(repository.root(), scratch.path("plain"));
    assert!(repository.head().unwrap().is_none());
    assert!(matches!(
        git.init(&scratch.path("missing"), Policy::Trusted),
        Err(Error::NotARepository { .. })
    ));
}

#[test]
fn a_missing_git_program_is_a_typed_error() {
    let scratch = Scratch::new("no-git");
    let git = term_git::Git::new(
        &term_process::Supervisor::new(),
        vec![("PATH".into(), scratch.path("home").to_string_lossy().into())],
    );
    // macOS always has /usr/bin/git as a fallback; the test only holds where it is absent.
    if git.program().is_none() {
        assert!(matches!(git.version(), Err(Error::GitMissing)));
    } else {
        assert!(git.version().unwrap().starts_with("git version"));
    }
}

#[test]
fn status_reads_changes_renames_untracked_and_ignored_files_with_odd_names() {
    let scratch = Scratch::new("status");
    let repository = scratch.repository(
        "r",
        &[
            ("keep.txt", "keep\n"),
            ("old name.txt", "a\nb\nc\nd\n"),
            (".gitignore", "*.log\n"),
        ],
    );
    let root = repository.root();
    write(root, "keep.txt", "changed\n");
    scratch.raw(root, &["mv", "old name.txt", "new name.txt"]);
    write(root, "ünïcode dir/file with spaces.txt", "u\n");
    write(root, "new\nline.txt", "n\n");
    write(root, "noise.log", "ignored\n");
    write(root, "staged.txt", "s\n");
    scratch.raw(root, &["add", "staged.txt"]);

    let status = repository
        .status(StatusOptions {
            untracked: Untracked::All,
            ignored: true,
        })
        .unwrap();
    let entry = |path: &str| status.entries.iter().find(|e| e.path == Path::new(path)).cloned();
    let keep = entry("keep.txt").unwrap();
    assert_eq!(
        (keep.index, keep.worktree),
        (FileState::Unmodified, FileState::Modified)
    );
    let renamed = entry("new name.txt").unwrap();
    assert_eq!(
        renamed.kind,
        EntryKind::Renamed {
            from: "old name.txt".into(),
            score: 100
        }
    );
    assert_eq!(entry("staged.txt").unwrap().index, FileState::Added);
    assert_eq!(
        entry("ünïcode dir/file with spaces.txt").unwrap().kind,
        EntryKind::Untracked
    );
    assert_eq!(entry("new\nline.txt").unwrap().kind, EntryKind::Untracked);
    assert_eq!(entry("noise.log").unwrap().kind, EntryKind::Ignored);
    assert_eq!(status.branch.head.as_deref(), Some("main"));

    let folders = repository
        .status(StatusOptions {
            untracked: Untracked::Folders,
            ignored: false,
        })
        .unwrap();
    assert!(folders.entries.iter().any(|e| e.path == Path::new("ünïcode dir/")));
    assert!(folders.entries.iter().all(|e| e.kind != EntryKind::Ignored));
}

#[test]
fn status_reports_conflicts_upstream_distance_and_stash() {
    let scratch = Scratch::new("conflict");
    let repository = scratch.repository("r", &[("c.txt", "base\n")]);
    let root = repository.root();
    scratch.raw(&scratch.dir, &["clone", "--quiet", "--bare", "r", "remote.git"]);
    scratch.raw(root, &["remote", "add", "origin", "../remote.git"]);
    scratch.raw(root, &["fetch", "--quiet", "origin"]);
    scratch.raw(root, &["branch", "--set-upstream-to=origin/main"]);
    scratch.raw(root, &["switch", "--quiet", "-c", "other"]);
    write(root, "c.txt", "theirs\n");
    scratch.raw(root, &["commit", "--quiet", "-am", "theirs"]);
    scratch.raw(root, &["switch", "--quiet", "main"]);
    write(root, "c.txt", "ours\n");
    scratch.raw(root, &["commit", "--quiet", "-am", "ours"]);
    write(root, "stashed.txt", "x\n");
    scratch.raw(root, &["stash", "push", "--quiet", "--include-untracked"]);
    assert!(!scratch.try_raw(root, &["merge", "--quiet", "other"]));

    let status = repository.status(StatusOptions::default()).unwrap();
    assert_eq!(status.branch.upstream.as_deref(), Some("origin/main"));
    assert_eq!(status.branch.ahead_behind, Some((1, 0)));
    assert_eq!(status.stash, 1);
    let conflict = status.entries.iter().find(|e| e.path == Path::new("c.txt")).unwrap();
    assert_eq!(conflict.kind, EntryKind::Unmerged(Conflict::BothModified));
    let diff = repository.diff(term_git::DiffSource::Unstaged, &["c.txt"]).unwrap();
    assert_eq!(diff[0].change, term_git::Change::Unmerged);
    assert!(matches!(
        repository.commit("merge", term_git::CommitOptions::default()),
        Err(Error::Conflicts { ref paths, .. }) if paths == &[PathBuf::from("c.txt")]
    ));
}

#[test]
fn branches_are_created_switched_and_deleted_only_when_merged() {
    let scratch = Scratch::new("branches");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let root = repository.root();
    repository.switch_to_new_branch("feature/x", None).unwrap();
    assert_eq!(repository.current_branch().unwrap().as_deref(), Some("feature/x"));
    write(root, "a.txt", "feature\n");
    repository.stage(&["a.txt"]).unwrap();
    repository
        .commit("feature work", term_git::CommitOptions::default())
        .unwrap();
    repository.switch_branch("main").unwrap();
    assert_eq!(read(root, "a.txt"), "a\n");

    let branches = repository.branches().unwrap();
    let feature = branches.iter().find(|b| b.name == "feature/x").unwrap();
    assert!(!feature.current && !feature.remote);
    assert_eq!(feature.subject, "feature work");
    assert!(branches.iter().find(|b| b.name == "main").unwrap().current);

    assert!(matches!(
        repository.delete_branch("feature/x"),
        Err(Error::NotMerged { .. })
    ));
    assert!(matches!(
        repository.create_branch("bad name", None),
        Err(Error::InvalidName { .. })
    ));
    assert!(matches!(
        repository.create_branch("-x", None),
        Err(Error::InvalidName { .. })
    ));
    repository.destructive().delete_branch("feature/x").unwrap();
    assert!(repository.branches().unwrap().iter().all(|b| b.name != "feature/x"));
}

#[test]
fn switching_over_local_changes_names_the_files_and_changes_nothing() {
    let scratch = Scratch::new("switch-dirty");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let root = repository.root();
    repository.switch_to_new_branch("other", None).unwrap();
    write(root, "a.txt", "other\n");
    repository.stage(&["a.txt"]).unwrap();
    repository.commit("other", term_git::CommitOptions::default()).unwrap();
    repository.switch_branch("main").unwrap();
    write(root, "a.txt", "mine\n");
    match repository.switch_branch("other") {
        Err(Error::LocalChanges { paths, .. }) => assert_eq!(paths, [PathBuf::from("a.txt")]),
        other => panic!("{other:?}"),
    }
    assert_eq!(read(root, "a.txt"), "mine\n");
    assert_eq!(repository.current_branch().unwrap().as_deref(), Some("main"));
}

#[test]
fn a_remote_branch_is_switched_to_through_a_tracking_branch() {
    let scratch = Scratch::new("track");
    let upstream = scratch.repository("up", &[("a.txt", "a\n")]);
    scratch.raw(upstream.root(), &["branch", "topic"]);
    scratch.raw(&scratch.dir, &["clone", "--quiet", "up", "down"]);
    let down = scratch.git().discover(&scratch.path("down"), Policy::Trusted).unwrap();
    down.switch_branch("origin/topic").unwrap();
    assert_eq!(down.current_branch().unwrap().as_deref(), Some("topic"));
    let topic = down
        .branches()
        .unwrap()
        .into_iter()
        .find(|b| b.name == "topic")
        .unwrap();
    assert_eq!(topic.upstream.as_deref(), Some("origin/topic"));
    assert!(
        down.branches()
            .unwrap()
            .iter()
            .any(|b| b.remote && b.name == "origin/topic")
    );
}

#[test]
fn the_log_lists_the_newest_commits_with_refs() {
    let scratch = Scratch::new("log");
    let repository = scratch.repository("r", &[("a.txt", "1\n")]);
    let root = repository.root();
    for n in 2..=4 {
        write(root, "a.txt", &format!("{n}\n"));
        scratch.raw(root, &["commit", "--quiet", "-am", &format!("commit {n}")]);
    }
    scratch.raw(root, &["tag", "v1"]);
    let log = repository.log(2).unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].subject, "commit 4");
    assert_eq!(log[0].author_name, "Test");
    assert!(log[0].refs.iter().any(|r| r == "HEAD -> main"));
    assert!(log[0].refs.iter().any(|r| r == "tag: v1"));
    assert_eq!(log[0].parents, [log[1].hash.clone()]);
    let empty = scratch.repository("empty", &[]);
    assert!(empty.log(5).unwrap().is_empty());
}

#[test]
fn the_history_takes_in_a_second_line_and_a_commit_tells_what_it_changed() {
    let scratch = Scratch::new("history");
    let repository = scratch.repository("r", &[("a.txt", "1\n2\n")]);
    let root = repository.root();
    scratch.raw(root, &["branch", "side"]);
    write(root, "a.txt", "1\nzwei\n");
    write(root, "b.txt", "neu\n");
    scratch.raw(root, &["add", "."]);
    scratch.raw(
        root,
        &["commit", "--quiet", "-m", "main work", "-m", "Why it was done."],
    );
    scratch.raw(root, &["switch", "--quiet", "side"]);
    write(root, "c.txt", "c\n");
    scratch.raw(root, &["add", "."]);
    scratch.raw(root, &["commit", "--quiet", "-m", "side work"]);
    scratch.raw(root, &["switch", "--quiet", "main"]);
    let only_head = repository.history(10, None).unwrap();
    assert_eq!(only_head.len(), 2);
    let both = repository.history(10, Some("side")).unwrap();
    assert_eq!(both.len(), 3);
    let root_commit = both.last().unwrap();
    assert!(root_commit.parents.is_empty());
    let work = both.iter().find(|c| c.subject == "main work").unwrap();
    assert_eq!(work.body, "Why it was done.");
    let changes = repository.commit_changes(&work.hash).unwrap();
    let files: Vec<_> = changes.files.iter().map(|f| (f.path.clone(), f.change)).collect();
    assert_eq!(
        files,
        [
            (PathBuf::from("a.txt"), Change::Modified),
            (PathBuf::from("b.txt"), Change::Added)
        ]
    );
    assert_eq!((changes.insertions, changes.deletions), (2, 1));
    let first = repository.commit_changes(&root_commit.hash).unwrap();
    assert_eq!(first.files.len(), 1);
    let diff = repository.diff_commit(&work.hash, Path::new("a.txt")).unwrap().unwrap();
    assert_eq!(diff.hunks.len(), 1);
    assert!(
        repository
            .diff_commit(&work.hash, Path::new("c.txt"))
            .unwrap()
            .is_none()
    );
    assert!(repository.history(10, Some("--all")).is_err());
}

#[test]
fn remotes_list_their_urls() {
    let scratch = Scratch::new("remotes");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    scratch.raw(
        repository.root(),
        &["remote", "add", "origin", "git@gitlab.example.com:team/app.git"],
    );
    scratch.raw(
        repository.root(),
        &["remote", "add", "hub", "https://github.com/team/app.git"],
    );
    scratch.raw(
        repository.root(),
        &["remote", "set-url", "--push", "hub", "https://push.example/app.git"],
    );
    let remotes = repository.remotes().unwrap();
    let origin = remotes.iter().find(|r| r.name == "origin").unwrap();
    assert_eq!(origin.fetch_url.as_deref(), Some("git@gitlab.example.com:team/app.git"));
    let hub = remotes.iter().find(|r| r.name == "hub").unwrap();
    assert_eq!(hub.push_urls, ["https://push.example/app.git"]);
}

#[test]
fn a_worktree_is_added_on_a_new_branch_listed_and_removed() {
    let scratch = Scratch::new("worktree");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    let place = scratch.path("trees/topic 1");
    let worktree = repository.add_worktree(&place, "topic-1", None).unwrap();
    assert!(worktree.is_linked_worktree());
    assert_eq!(worktree.root(), place);
    assert_eq!(worktree.common_dir(), repository.common_dir());
    assert_eq!(worktree.current_branch().unwrap().as_deref(), Some("topic-1"));

    let listed = repository.worktrees().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed[0].main);
    assert_eq!(listed[1].path, place);
    assert_eq!(listed[1].branch.as_deref(), Some("refs/heads/topic-1"));

    write(&place, "dirty.txt", "x\n");
    assert!(matches!(
        repository.remove_worktree(&place),
        Err(Error::LocalChanges { .. })
    ));
    repository.destructive().remove_worktree(&place).unwrap();
    assert!(!place.exists());
    assert_eq!(repository.worktrees().unwrap().len(), 1);

    let other = scratch.path("trees/other");
    repository.add_worktree(&other, "other", None).unwrap();
    std::fs::remove_dir_all(&other).unwrap();
    assert!(repository.worktrees().unwrap()[1].prunable.is_some());
    repository.prune_worktrees().unwrap();
    assert_eq!(repository.worktrees().unwrap().len(), 1);
}

#[test]
fn reset_and_clean_throw_away_changes_and_untracked_files_but_not_ignored_ones() {
    let scratch = Scratch::new("destructive");
    let repository = scratch.repository("r", &[("a.txt", "a\n"), (".gitignore", "*.log\n")]);
    let root = repository.root();
    write(root, "a.txt", "changed\n");
    write(root, "junk/new file.txt", "x\n");
    write(root, "keep.log", "x\n");
    repository.destructive().reset_hard("HEAD").unwrap();
    assert_eq!(read(root, "a.txt"), "a\n");
    let preview = repository.destructive().clean_preview().unwrap();
    assert_eq!(preview, [PathBuf::from("junk/")]);
    assert!(root.join("junk/new file.txt").exists());
    assert_eq!(repository.destructive().clean().unwrap(), preview);
    assert!(!root.join("junk").exists());
    assert!(root.join("keep.log").exists());
    assert!(matches!(
        repository.destructive().reset_hard("--hard"),
        Err(Error::InvalidName { .. })
    ));
}
