//! Fills over made-up build folders whose files carry the times a build would have left on them.

use std::collections::HashSet;
use std::fs::{self, File, FileTimes};

use super::*;
use crate::shell::fake::scratch;

fn days_ago(now: SystemTime, days: u64) -> SystemTime {
    now - Duration::from_secs(days * 24 * 60 * 60)
}

/// A file a build last read at `read` and wrote at `written`, with its folders.
fn file(path: &Path, read: SystemTime, written: SystemTime) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"x").unwrap();
    let times = FileTimes::new().set_accessed(read).set_modified(written);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

/// Gives `dir` and every folder in it the time `time`, as if nothing had changed them since.
fn settle(dir: &Path, time: SystemTime) {
    for sub in subdirs(dir) {
        settle(&sub, time);
    }
    let times = FileTimes::new().set_accessed(time).set_modified(time);
    File::open(dir).unwrap().set_times(times).unwrap();
}

/// The build folder of the workspace whose path hash is `hash`, with cargo's tag.
fn folder(root: &Path, hash: &str) -> PathBuf {
    let folder = root.join(&hash[..2]).join(&hash[2..]);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    folder
}

/// A unit in cargo 1.100's layout whose fingerprint a build last read at `read`.
fn unit(dest: &Path, package: &str, hash: &str, read: SystemTime, written: SystemTime) -> PathBuf {
    let unit = dest.join("build").join(package).join(hash);
    file(&unit.join("fingerprint/lib-x"), read, written);
    file(&unit.join("out/libx.rlib"), written, written);
    unit
}

#[test]
fn the_recent_units_of_registry_packages_are_cloned_with_their_times_and_the_folder_names_its_worktree() {
    let now = SystemTime::now();
    let root = scratch("seed-fill");
    let from = folder(&root, "7b95d91361e16a8e");
    let source = from.join("debug");
    let recent = unit(&source, "serde", "faba822365272399", now, days_ago(now, 3));
    unit(&source, "serde", "912dc2cb26f1601d", days_ago(now, 9), days_ago(now, 9));
    unit(&source, "term-base", "a208be26cafb1dcc", now, now);
    file(&source.join("incremental/term_base-1abc/s-1.lock"), now, now);
    std::os::unix::fs::symlink("libx.rlib", recent.join("out/libx.a")).unwrap();
    settle(&root.join("7b"), days_ago(now, 20));
    let to = root.join("d5/df618857e9c689");
    let worktree = root.join("worktree");
    let packages = HashSet::from(["serde".to_string()]);

    assert_eq!(fill(&from, &to, &worktree, &packages), Ok(Some(1)));

    let clone = to.join("debug/build/serde/faba822365272399");
    let written = |path: &Path| fs::metadata(path).unwrap().modified().unwrap();
    assert_eq!(
        written(&clone.join("fingerprint/lib-x")),
        written(&recent.join("fingerprint/lib-x")),
        "cargo compares the times of a unit's files"
    );
    assert!(clone.join("out/libx.rlib").is_file());
    assert_eq!(fs::read_link(clone.join("out/libx.a")).unwrap(), Path::new("libx.rlib"));
    assert!(
        !to.join("debug/build/serde/912dc2cb26f1601d").exists(),
        "of an earlier compiler"
    );
    assert!(
        !to.join("debug/build/term-base").exists(),
        "the worktree builds its own crates"
    );
    assert!(!to.join("debug/incremental").exists());
    assert!(
        to.join("CACHEDIR.TAG").is_file(),
        "a sweep finds the folder by cargo's tag"
    );
    assert_eq!(fs::read(to.join(WORKTREE)).unwrap(), worktree.as_os_str().as_bytes());
    assert_eq!(
        fill(&from, &to, &worktree, &packages),
        Ok(Some(0)),
        "nothing is cloned twice"
    );
}

#[test]
fn a_build_in_the_main_checkout_leaves_the_worktree_to_build_its_dependencies_itself() {
    let now = SystemTime::now();
    let root = scratch("seed-busy");
    let from = folder(&root, "7b95d91361e16a8e");
    unit(&from.join("debug"), "serde", "faba822365272399", now, now);
    file(&from.join("debug/.cargo-build-lock"), now, now);
    let held = File::options()
        .read(true)
        .write(true)
        .open(from.join("debug/.cargo-build-lock"))
        .unwrap();
    held.try_lock().unwrap();
    let to = root.join("d5/df618857e9c689");
    let packages = HashSet::from(["serde".to_string()]);

    assert_eq!(fill(&from, &to, &root.join("worktree"), &packages), Ok(None));
    assert!(!to.join("debug/build").exists());
    drop(held);
}

#[test]
fn a_checkout_that_never_built_has_nothing_to_clone() {
    let root = scratch("seed-empty");
    let from = folder(&root, "7b95d91361e16a8e");
    let to = root.join("d5/df618857e9c689");
    assert_eq!(fill(&from, &to, &root.join("worktree"), &HashSet::new()), Ok(Some(0)));
    assert!(!to.exists());
}

#[test]
fn a_worktree_is_found_from_any_folder_in_it_and_only_when_it_belongs_to_the_checkout() {
    let root = scratch("seed-worktree").canonicalize().unwrap();
    let main = root.join("tapa");
    let git_dir = main.join(".git/worktrees/agent-1");
    fs::create_dir_all(&git_dir).unwrap();
    let worktree = main.join(".claude/worktrees/agent-1");
    fs::create_dir_all(worktree.join("crates/term-base")).unwrap();
    fs::write(worktree.join(".git"), format!("gitdir: {}\n", git_dir.display())).unwrap();
    let relative = root.join("relative");
    fs::create_dir_all(&relative).unwrap();
    fs::write(relative.join(".git"), "gitdir: ../tapa/.git/worktrees/agent-1\n").unwrap();
    let foreign = root.join("other/.git/worktrees/x");
    fs::create_dir_all(&foreign).unwrap();
    let elsewhere = root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join(".git"), format!("gitdir: {}\n", foreign.display())).unwrap();

    let found = Some((worktree.clone(), git_dir.clone()));
    assert_eq!(linked_worktree(&worktree.join("crates/term-base"), &main), found);
    assert_eq!(linked_worktree(&relative, &main), Some((relative, git_dir)));
    assert_eq!(
        linked_worktree(&main, &main),
        None,
        "the main checkout has a folder .git"
    );
    assert_eq!(
        linked_worktree(&elsewhere, &main),
        None,
        "a worktree of another repository"
    );
}
