//! Staging, unstaging and discarding files and hunks, checked against what git itself then reports.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::path::{Path, PathBuf};

use common::{Scratch, numbered, read, write, write_bytes};
use term_git::{Change, DiffSource, Error, FileDiff, LineKind, Repository, StatusOptions};

/// A file of 40 lines with three separate changes, so the diff has three hunks.
fn three_changes() -> String {
    numbered(40)
        .replace("line 3\n", "line 3 changed\n")
        .replace("line 20\n", "line 20\nline 20.5 added\n")
        .replace("line 37\n", "")
}

fn index_content(scratch: &Scratch, repository: &Repository, path: &str) -> String {
    scratch.raw(repository.root(), &["show", &format!(":{path}")])
}

fn head_content(scratch: &Scratch, repository: &Repository, path: &str) -> String {
    scratch.raw(repository.root(), &["show", &format!("HEAD:{path}")])
}

fn only(diffs: Vec<FileDiff>) -> FileDiff {
    assert_eq!(diffs.len(), 1, "{diffs:?}");
    diffs.into_iter().next().unwrap()
}

#[test]
fn every_subset_of_hunks_stages_and_unstages_back_to_the_start() {
    let scratch = Scratch::new("subsets");
    let repository = scratch.repository("r", &[("f.txt", &numbered(40))]);
    let root = repository.root();
    write(root, "f.txt", &three_changes());
    for subset in 1u32..8 {
        let chosen: Vec<usize> = (0..3).filter(|i| subset & (1 << i) != 0).collect();
        let unstaged = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
        assert_eq!(unstaged.hunks.len(), 3);
        repository.stage_hunks(&unstaged, &chosen).unwrap();

        let staged = only(repository.diff(DiffSource::Staged, &["f.txt"]).unwrap());
        assert_eq!(staged.hunks.len(), chosen.len(), "subset {chosen:?}");
        let left = repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap();
        assert_eq!(
            left.iter().map(|d| d.hunks.len()).sum::<usize>(),
            3 - chosen.len(),
            "subset {chosen:?}"
        );
        assert_eq!(read(root, "f.txt"), three_changes(), "the working tree stays as it is");

        // Unstage one hunk at a time, the last first, each from a fresh diff.
        while let Some(staged) = repository
            .diff(DiffSource::Staged, &["f.txt"])
            .unwrap()
            .into_iter()
            .next()
        {
            let last = staged.hunks.len() - 1;
            repository.unstage_hunks(&staged, &[last]).unwrap();
        }
        assert_eq!(
            index_content(&scratch, &repository, "f.txt"),
            numbered(40),
            "subset {chosen:?}"
        );
    }
}

#[test]
fn staging_the_rest_hunk_by_hunk_makes_the_index_match_the_file() {
    let scratch = Scratch::new("rest");
    let repository = scratch.repository("r", &[("f.txt", &numbered(40))]);
    write(repository.root(), "f.txt", &three_changes());
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    repository.stage_hunks(&diff, &[1]).unwrap();
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    repository.stage_hunks(&diff, &[1]).unwrap();
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    repository.stage_hunks(&diff, &[0]).unwrap();
    assert!(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap().is_empty());
    assert_eq!(index_content(&scratch, &repository, "f.txt"), three_changes());
}

#[test]
fn discarding_a_hunk_takes_only_that_change_out_of_the_file() {
    let scratch = Scratch::new("discard-hunk");
    let repository = scratch.repository("r", &[("f.txt", &numbered(40))]);
    let root = repository.root();
    write(root, "f.txt", &three_changes());
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    repository.discard_hunks(&diff, &[1]).unwrap();
    let expected = numbered(40)
        .replace("line 3\n", "line 3 changed\n")
        .replace("line 37\n", "");
    assert_eq!(read(root, "f.txt"), expected);
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    repository.discard_hunks(&diff, &[0, 1]).unwrap();
    assert_eq!(read(root, "f.txt"), numbered(40));
}

#[test]
fn a_stale_diff_is_refused_without_changing_anything() {
    let scratch = Scratch::new("stale");
    let repository = scratch.repository("r", &[("f.txt", &numbered(10))]);
    let root = repository.root();
    write(root, "f.txt", &numbered(10).replace("line 5\n", "line 5 first\n"));
    let diff = only(repository.diff(DiffSource::Unstaged, &["f.txt"]).unwrap());
    write(root, "f.txt", &numbered(10).replace("line 5\n", "line 5 second\n"));
    assert!(matches!(
        repository.discard_hunks(&diff, &[0]),
        Err(Error::PatchDoesNotApply { .. })
    ));
    assert_eq!(read(root, "f.txt"), numbered(10).replace("line 5\n", "line 5 second\n"));
    assert!(matches!(
        repository.stage_hunks(&diff, &[7]),
        Err(Error::Unexpected { .. })
    ));
}

#[test]
fn hunks_keep_crlf_line_ends_missing_final_newlines_and_invalid_utf8() {
    let scratch = Scratch::new("bytes");
    let original: &[u8] = b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nten";
    let repository = scratch.repository("r", &[("placeholder", "x\n")]);
    let root = repository.root();
    write_bytes(root, "crlf.txt", original);
    scratch.raw(root, &["add", "crlf.txt"]);
    scratch.raw(root, &["commit", "--quiet", "-m", "crlf"]);
    let changed: &[u8] = b"one\r\nTWO \xff\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nten!";
    write_bytes(root, "crlf.txt", changed);
    let diff = only(repository.diff(DiffSource::Unstaged, &["crlf.txt"]).unwrap());
    assert_eq!(diff.hunks.len(), 2);
    let last = diff.hunks[1].lines.last().unwrap();
    assert!(last.missing_newline && last.kind == LineKind::Added && last.content == b"ten!");
    assert!(diff.hunks[0].lines.iter().any(|l| l.content == b"TWO \xff\r"));
    repository.stage_hunks(&diff, &[1]).unwrap();
    let staged = scratch.raw(root, &["show", ":crlf.txt"]);
    assert!(staged.ends_with("nine\r\nten!"));
    assert!(staged.contains("two\r\n"));
    let staged_diff = only(repository.diff(DiffSource::Staged, &["crlf.txt"]).unwrap());
    repository.unstage_hunks(&staged_diff, &[0]).unwrap();
    assert!(repository.diff(DiffSource::Staged, &["crlf.txt"]).unwrap().is_empty());
    let diff = only(repository.diff(DiffSource::Unstaged, &["crlf.txt"]).unwrap());
    repository.discard_hunks(&diff, &[0, 1]).unwrap();
    assert_eq!(std::fs::read(root.join("crlf.txt")).unwrap(), original);
}

#[test]
fn files_with_spaces_newlines_and_unicode_in_their_names_stage_by_hunk() {
    let scratch = Scratch::new("names");
    let names = [
        "with space.txt",
        "new\nline.txt",
        "ümlaut ö.txt",
        "quo\"te\\.txt",
        "*star?.txt",
    ];
    let files: Vec<(&str, String)> = names.iter().map(|n| (*n, numbered(12))).collect();
    let files: Vec<(&str, &str)> = files.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let repository = scratch.repository("r", &files);
    for name in names {
        write(
            repository.root(),
            name,
            &numbered(12)
                .replace("line 2\n", "two\n")
                .replace("line 11\n", "eleven\n"),
        );
        let diff = only(repository.diff(DiffSource::Unstaged, &[name]).unwrap());
        assert_eq!(diff.path(), Path::new(name));
        repository.stage_hunks(&diff, &[1]).unwrap();
        let staged = index_content(&scratch, &repository, name);
        assert!(staged.contains("eleven") && !staged.contains("two"), "{name}");
        let staged = only(repository.diff(DiffSource::Staged, &[name]).unwrap());
        repository.unstage_hunks(&staged, &[0]).unwrap();
        assert_eq!(index_content(&scratch, &repository, name), numbered(12), "{name}");
    }
}

#[test]
fn an_untracked_file_is_staged_as_a_new_file_and_unstaged_away_again() {
    let scratch = Scratch::new("new-file");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write(repository.root(), "fresh.txt", "one\ntwo\n");
    let diff = repository.diff_untracked(Path::new("fresh.txt")).unwrap();
    assert_eq!(diff.change, Change::Added);
    assert_eq!(diff.hunks[0].lines.len(), 2);
    assert!(matches!(
        repository.discard_hunks(&diff, &[0]),
        Err(Error::Untracked { .. })
    ));
    repository.stage_hunks(&diff, &[0]).unwrap();
    assert_eq!(index_content(&scratch, &repository, "fresh.txt"), "one\ntwo\n");
    let staged = only(repository.diff(DiffSource::Staged, &["fresh.txt"]).unwrap());
    assert_eq!(staged.change, Change::Added);
    repository.unstage_hunks(&staged, &[0]).unwrap();
    assert!(!scratch.try_raw(repository.root(), &["show", ":fresh.txt"]));
    assert!(repository.root().join("fresh.txt").exists());
}

#[test]
fn a_deleted_file_is_staged_and_unstaged_as_one_hunk() {
    let scratch = Scratch::new("deleted");
    let repository = scratch.repository("r", &[("gone.txt", "a\nb\n"), ("stay.txt", "s\n")]);
    std::fs::remove_file(repository.root().join("gone.txt")).unwrap();
    let diff = only(repository.diff(DiffSource::Unstaged, &["gone.txt"]).unwrap());
    assert_eq!(diff.change, Change::Deleted);
    repository.stage_hunks(&diff, &[0]).unwrap();
    assert!(!scratch.try_raw(repository.root(), &["show", ":gone.txt"]));
    let staged = only(repository.diff(DiffSource::Staged, &["gone.txt"]).unwrap());
    repository.unstage_hunks(&staged, &[0]).unwrap();
    assert_eq!(index_content(&scratch, &repository, "gone.txt"), "a\nb\n");
}

#[test]
fn a_hunk_of_a_staged_rename_is_unstaged_without_undoing_the_rename() {
    let scratch = Scratch::new("rename");
    let repository = scratch.repository("r", &[("old.txt", &numbered(30))]);
    let root = repository.root();
    scratch.raw(root, &["mv", "old.txt", "new.txt"]);
    write(
        root,
        "new.txt",
        &numbered(30)
            .replace("line 2\n", "two\n")
            .replace("line 29\n", "twenty-nine\n"),
    );
    repository.stage(&["new.txt"]).unwrap();
    let status = repository.status(StatusOptions::default()).unwrap();
    let entry = status.entries.iter().find(|e| e.path == Path::new("new.txt")).unwrap();
    let staged = repository.diff_entry(entry, DiffSource::Staged).unwrap().unwrap();
    assert!(matches!(staged.change, Change::Renamed { .. }));
    assert_eq!(staged.old_path.as_deref(), Some(Path::new("old.txt")));
    repository.unstage_hunks(&staged, &[0]).unwrap();
    let index = index_content(&scratch, &repository, "new.txt");
    assert!(index.contains("line 2\n") && index.contains("twenty-nine"));
    assert!(!scratch.try_raw(root, &["show", ":old.txt"]));
}

#[test]
fn binary_files_have_no_hunks_to_stage() {
    let scratch = Scratch::new("binary");
    let repository = scratch.repository("r", &[("a.txt", "a\n")]);
    write_bytes(repository.root(), "a.txt", b"\0\x01\x02binary");
    let diff = only(repository.diff(DiffSource::Unstaged, &["a.txt"]).unwrap());
    assert!(diff.binary);
    assert!(matches!(repository.stage_hunks(&diff, &[0]), Err(Error::Binary { .. })));
}

#[test]
fn whole_files_are_staged_unstaged_and_discarded_but_untracked_ones_are_refused() {
    let scratch = Scratch::new("files");
    let repository = scratch.repository("r", &[("a.txt", "a\n"), ("b.txt", "b\n"), ("*.txt", "star\n")]);
    let root = repository.root();
    write(root, "a.txt", "A\n");
    write(root, "b.txt", "B\n");
    write(root, "*.txt", "STAR\n");
    std::fs::remove_file(root.join("b.txt")).unwrap();
    write(root, "new.txt", "n\n");
    repository.stage(&["*.txt"]).unwrap();
    assert_eq!(index_content(&scratch, &repository, "*.txt"), "STAR\n");
    assert_eq!(
        index_content(&scratch, &repository, "a.txt"),
        "a\n",
        "a pathspec with * matches only itself"
    );
    repository.stage(&[root.join("a.txt"), PathBuf::from("b.txt")]).unwrap();
    assert!(!scratch.try_raw(root, &["show", ":b.txt"]));
    repository.unstage(&["a.txt", "b.txt"]).unwrap();
    assert_eq!(index_content(&scratch, &repository, "a.txt"), "a\n");
    assert_eq!(index_content(&scratch, &repository, "b.txt"), "b\n");

    match repository.discard(&["a.txt", "new.txt"]) {
        Err(Error::Untracked { paths }) => assert_eq!(paths, [PathBuf::from("new.txt")]),
        other => panic!("{other:?}"),
    }
    assert_eq!(read(root, "a.txt"), "A\n", "nothing changes when a file is refused");
    repository.discard(&["a.txt", "b.txt"]).unwrap();
    assert_eq!(read(root, "a.txt"), "a\n");
    assert_eq!(read(root, "b.txt"), "b\n");
    assert_eq!(read(root, "*.txt"), "STAR\n", "staged changes stay");
    assert!(matches!(
        repository.stage(&["../outside.txt"]),
        Err(Error::OutsideFolder { .. })
    ));

    repository.stage_all().unwrap();
    assert_eq!(index_content(&scratch, &repository, "new.txt"), "n\n");
    repository.unstage_all().unwrap();
    assert!(repository.diff(DiffSource::Staged, &[] as &[&str]).unwrap().is_empty());
    assert_eq!(head_content(&scratch, &repository, "a.txt"), "a\n");
}

#[test]
fn before_the_first_commit_diffs_compare_against_nothing() {
    let scratch = Scratch::new("unborn");
    let repository = scratch.repository("r", &[]);
    write(repository.root(), "a.txt", "a\n");
    repository.stage(&["a.txt"]).unwrap();
    write(repository.root(), "a.txt", "a\nb\n");
    let against_head = only(repository.diff(DiffSource::AgainstHead, &["a.txt"]).unwrap());
    assert_eq!(against_head.change, Change::Added);
    assert_eq!(against_head.hunks[0].new_count, 2);
    let staged = only(repository.diff(DiffSource::Staged, &["a.txt"]).unwrap());
    assert_eq!(staged.hunks[0].new_count, 1);
    repository.unstage(&["a.txt"]).unwrap();
    let status = repository.status(StatusOptions::default()).unwrap();
    assert_eq!(status.branch.commit, None);
    let entry = &status.entries[0];
    let diff = repository.diff_entry(entry, DiffSource::Unstaged).unwrap().unwrap();
    assert_eq!(diff.change, Change::Added);
    assert_eq!(diff.hunks[0].lines.len(), 2);
}
