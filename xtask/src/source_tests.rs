//! The source of a deploy against real git: repositories in a scratch folder, a bare one as origin, and no
//! configuration of this Mac.

use std::path::{Path, PathBuf};

use super::*;
use crate::shell::fake::scratch;

/// A repository with one commit of `a.txt`, a `Cargo.toml` and `Cargo.lock` at version 0.1.0 and a `.gitignore` for
/// `ignored.txt`, and a bare origin.
pub(crate) fn repository(name: &str) -> (PathBuf, Git) {
    let dir = scratch(name);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let home = home.display().to_string();
    let mut git = Git::at(&dir.join("repo"));
    git.env = [
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("HOME", home.as_str()),
        ("XDG_CONFIG_HOME", home.as_str()),
        ("GIT_AUTHOR_NAME", "Test"),
        ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
        ("GIT_COMMITTER_NAME", "Test"),
        ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
    ]
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .to_vec();
    let origin = dir.join("origin.git").display().to_string();
    let bare = Git {
        dir: dir.clone(),
        env: git.env.clone(),
    };
    bare.output(&mut System, &["init", "--quiet", "--bare", "-b", BRANCH, &origin])
        .unwrap();
    bare.output(&mut System, &["init", "--quiet", "-b", BRANCH, "repo"])
        .unwrap();
    write(&git, "a.txt", "a\n");
    write(&git, version::MANIFEST, version::tests::MANIFEST_TEXT);
    write(&git, version::LOCK, version::tests::LOCK_TEXT);
    write(&git, ".gitignore", "ignored.txt\n");
    commit(&git, "first");
    git.output(&mut System, &["remote", "add", "origin", &origin]).unwrap();
    (dir, git)
}

fn write(git: &Git, file: &str, content: &str) {
    std::fs::write(git.dir().join(file), content).unwrap();
}

fn commit(git: &Git, message: &str) {
    git.output(&mut System, &["add", "--all"]).unwrap();
    git.output(&mut System, &["commit", "--quiet", "--no-gpg-sign", "-m", message])
        .unwrap();
}

/// The sibling `name` beside `git`'s repository in `dir`, with one commit of `lib.rs` pushed to a bare origin it tracks.
fn sibling(dir: &Path, git: &Git, name: &str) -> Git {
    let sibling = git.sibling(name);
    let origin = dir.join(format!("{name}-origin.git")).display().to_string();
    let parent = Git {
        dir: dir.to_path_buf(),
        env: git.env.clone(),
    };
    parent
        .output(&mut System, &["init", "--quiet", "--bare", "-b", BRANCH, &origin])
        .unwrap();
    parent
        .output(&mut System, &["init", "--quiet", "-b", BRANCH, name])
        .unwrap();
    write(&sibling, "lib.rs", "fn a() {}\n");
    commit(&sibling, "first");
    sibling
        .output(&mut System, &["remote", "add", "origin", &origin])
        .unwrap();
    sibling
        .output(&mut System, &["push", "--quiet", "-u", "origin", BRANCH])
        .unwrap();
    sibling
}

fn head(git: &Git) -> String {
    git.output(&mut System, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string()
}

fn fp(git: &Git, content: Content) -> String {
    fingerprint(&mut System, git, content).unwrap()
}

fn origin_refs(dir: &Path) -> String {
    let origin = Git::at(&dir.join("origin.git"));
    origin
        .output(&mut System, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .unwrap()
}

#[test]
fn the_fingerprint_is_the_content_without_version() {
    let (dir, git) = repository("source-fingerprint");
    let first = fp(&git, Content::WorkingTree);
    assert!(is_object_name(&first), "{first}");
    assert_eq!(fp(&git, Content::Head), first, "a clean tree is HEAD's content");
    assert_eq!(
        fp(&git, Content::WorkingTree),
        first,
        "the same content, the same fingerprint"
    );

    version::write(&mut System, &git, "0.1.1").unwrap();
    assert_eq!(
        fp(&git, Content::WorkingTree),
        first,
        "a raised build number is no change"
    );
    write(&git, "ignored.txt", "scratch\n");
    assert_eq!(fp(&git, Content::WorkingTree), first, "ignored files are no content");

    write(&git, "new.txt", "new\n");
    let untracked = fp(&git, Content::WorkingTree);
    assert_ne!(untracked, first, "an untracked file is content");
    assert_eq!(fp(&git, Content::Head), first);
    let status = git.output(&mut System, &["status", "--porcelain"]).unwrap();
    assert_eq!(
        status, " M Cargo.lock\n M Cargo.toml\n?? new.txt\n",
        "the fingerprint leaves the index alone"
    );
    std::fs::remove_file(git.dir().join("new.txt")).unwrap();
    write(&git, "a.txt", "changed\n");
    let changed = fp(&git, Content::WorkingTree);
    assert!(changed != first && changed != untracked);

    write(&git, "a.txt", "a\n");
    commit(&git, "only the version");
    assert_eq!(
        fp(&git, Content::Head),
        first,
        "a commit of the raised build number keeps the fingerprint"
    );

    std::fs::remove_dir_all(git.dir().join(".git")).unwrap();
    assert!(
        fingerprint(&mut System, &git, Content::Head).is_err(),
        "no repository, no fingerprint"
    );
    assert!(!is_object_name("abc") && !is_object_name(&"g".repeat(40)) && is_object_name(&"a".repeat(64)));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_fingerprint_follows_the_siblings_commits_and_changes() {
    let (dir, git) = repository("source-siblings");
    let alone = fp(&git, Content::WorkingTree);
    let splat = sibling(&dir, &git, "splat");
    let with = fp(&git, Content::WorkingTree);
    assert_ne!(with, alone, "a sibling is content");
    assert_eq!(fp(&git, Content::Head), with, "of HEAD's content too");
    let listed = git.output(&mut System, &["ls-tree", "-r", &with]).unwrap();
    assert!(
        listed.contains(&format!("160000 commit {}\t.siblings/splat\n", head(&splat))),
        "{listed}"
    );

    write(&splat, "lib.rs", "fn b() {}\n");
    let changed = fp(&git, Content::WorkingTree);
    assert_ne!(changed, with, "a change not committed there is content");
    write(&splat, "lib.rs", "fn c() {}\n");
    let again = fp(&git, Content::WorkingTree);
    assert!(
        again != changed && again != with,
        "so is a further change of the same file"
    );
    assert_eq!(
        splat.output(&mut System, &["status", "--porcelain"]).unwrap(),
        " M lib.rs\n",
        "the sibling's index stays as it was"
    );
    write(&splat, "lib.rs", "fn a() {}\n");
    assert_eq!(
        fp(&git, Content::WorkingTree),
        with,
        "the same state, the same fingerprint"
    );

    write(&splat, "lib.rs", "fn d() {}\n");
    commit(&splat, "second");
    let committed = fp(&git, Content::WorkingTree);
    assert!(
        committed != with && committed != alone,
        "a new commit there is new content"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deploy_needs_clean_siblings_whose_commits_their_upstream_has() {
    let (dir, git) = repository("source-siblings-ready");
    let missing = siblings_ready(&mut System, &git).unwrap_err();
    assert!(missing.starts_with("../splat is missing"), "{missing}");
    let splat = sibling(&dir, &git, "splat");
    let lowspeak = sibling(&dir, &git, "lowspeak");
    let commits = siblings_ready(&mut System, &git).unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits["splat"], head(&splat));
    assert_eq!(commits["lowspeak"], head(&lowspeak));

    write(&lowspeak, "new.rs", "\n");
    let dirty = siblings_ready(&mut System, &git).unwrap_err();
    assert!(dirty.starts_with("../lowspeak has changes"), "{dirty}");
    commit(&lowspeak, "local");
    let ahead = siblings_ready(&mut System, &git).unwrap_err();
    assert!(
        ahead.starts_with("../lowspeak has commits that origin/main lacks"),
        "{ahead}"
    );
    lowspeak.output(&mut System, &["push", "--quiet"]).unwrap();
    assert_eq!(siblings_ready(&mut System, &git).unwrap()["lowspeak"], head(&lowspeak));

    splat.output(&mut System, &["branch", "--unset-upstream"]).unwrap();
    let untracked = siblings_ready(&mut System, &git).unwrap_err();
    assert!(untracked.contains("no upstream"), "{untracked}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deploy_pushes_main_and_its_tag_from_clean_main_only() {
    let (dir, git) = repository("source-deploy");
    let head = fp(&git, Content::Head);
    let refused = |git: &Git| ready(&mut System, git).unwrap_err();

    git.output(&mut System, &["switch", "--quiet", "-c", "topic"]).unwrap();
    assert!(
        refused(&git).starts_with("deploy pushes main, but \"topic\""),
        "on another branch"
    );
    git.output(&mut System, &["switch", "--quiet", BRANCH]).unwrap();

    write(&git, "new.txt", "new\n");
    assert!(refused(&git).contains("not committed"), "with an untracked file");
    assert!(push(&mut System, &git, "v0.1.0").is_err());
    assert!(!origin_refs(&dir).contains("refs/heads/main"), "nothing pushed");
    std::fs::remove_file(git.dir().join("new.txt")).unwrap();

    assert_eq!(ready(&mut System, &git), Ok(head));
    git.output(&mut System, &["tag", "v0.1.0"]).unwrap();
    assert_eq!(push(&mut System, &git, "v0.1.0"), Ok(()));
    let pushed = origin_refs(&dir);
    let main = git.output(&mut System, &["rev-parse", BRANCH]).unwrap();
    assert!(pushed.contains(&format!("refs/heads/main {}", main.trim())), "{pushed}");
    assert!(pushed.contains("refs/tags/v0.1.0"), "{pushed}");
    std::fs::remove_dir_all(dir).unwrap();
}
