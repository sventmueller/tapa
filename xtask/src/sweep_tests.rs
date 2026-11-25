//! Sweeps over made-up build folders whose files carry the times a build would have left on them.

use std::fs::{self, File, FileTimes};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::shell::fake::scratch;

pub(crate) const UNCAPPED: Policy = Policy {
    max_age: Duration::from_secs(7 * DAY),
    abandoned: Duration::from_secs(30 * DAY),
    keep: Duration::from_secs(2 * DAY),
    cap: u64::MAX,
};

pub(crate) fn days_ago(now: SystemTime, days: u64) -> SystemTime {
    now - Duration::from_secs(days * DAY)
}

/// A file a build last read at `read` and wrote at `written`, with its folders.
pub(crate) fn file(path: &Path, read: SystemTime, written: SystemTime) {
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
pub(crate) fn settle(dir: &Path, time: SystemTime) {
    for sub in subdirs(dir) {
        settle(&sub, time);
    }
    let times = FileTimes::new().set_accessed(time).set_modified(time);
    File::open(dir).unwrap().set_times(times).unwrap();
}

/// The build folder of the workspace whose path hash is `hash`, with cargo's tag.
pub(crate) fn folder(root: &Path, hash: &str) -> PathBuf {
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
pub(crate) fn unit(dest: &Path, package: &str, hash: &str, read: SystemTime, written: SystemTime) -> PathBuf {
    let unit = dest.join("build").join(package).join(hash);
    file(&unit.join("fingerprint/lib-x"), read, written);
    file(&unit.join("out/libx.rlib"), written, written);
    unit
}

#[test]
fn a_unit_its_workspace_has_not_read_for_a_week_goes_and_one_read_yesterday_stays() {
    let now = SystemTime::now();
    let root = scratch("sweep-age");
    let dest = folder(&root, "ef116096e23958fb").join("debug");
    let old = unit(&dest, "serde", "912dc2cb26f1601d", days_ago(now, 9), days_ago(now, 9));
    let used = unit(&dest, "serde", "faba822365272399", days_ago(now, 1), days_ago(now, 20));
    let lonely = unit(&dest, "ring", "c37901c3d831b362", days_ago(now, 9), days_ago(now, 9));
    let stale_cache = dest.join("incremental/serde-1abc");
    file(&stale_cache.join("s-1.lock"), days_ago(now, 9), days_ago(now, 9));
    let cache = dest.join("incremental/serde-2abc");
    file(&cache.join("s-2.lock"), now, now);
    settle(&root.join("ef"), days_ago(now, 20));

    let report = sweep_under(&root, UNCAPPED, now, false);

    assert!(!old.exists() && used.exists(), "a build read the second one yesterday");
    assert!(
        !lonely.parent().unwrap().exists(),
        "a package's folder goes with its last unit"
    );
    assert!(!stale_cache.exists() && cache.exists());
    assert_eq!(report.folders.len(), 1);
    assert_eq!(report.folders[0].0, "ef/116096e23958fb");
    assert_eq!(report.folders[0].1, 3);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(report.left > 0 && report.busy == 0);
}

#[test]
fn before_cargo_1_100_a_unit_goes_with_its_files_in_deps_and_its_build_script_folder() {
    let now = SystemTime::now();
    let root = scratch("sweep-old-layout");
    let dest = folder(&root, "ea315453a47da4ea").join("aarch64-apple-darwin/release");
    let paths = |hash: &str| {
        [
            format!(".fingerprint/serde-{hash}/lib-serde"),
            format!("deps/libserde-{hash}.rlib"),
            format!("deps/serde-{hash}.d"),
            format!("build/serde-{hash}/out/generated.rs"),
        ]
        .map(|path| dest.join(path))
    };
    for (hash, read) in [("515fb478708f4d1c", days_ago(now, 9)), ("0123456789abcdef", now)] {
        for path in paths(hash) {
            file(&path, read, read);
        }
    }
    settle(&root.join("ea"), days_ago(now, 20));

    sweep_under(&root, UNCAPPED, now, false);

    assert!(paths("515fb478708f4d1c").iter().all(|path| !path.exists()));
    assert!(paths("0123456789abcdef").iter().all(|path| path.exists()));
}

#[test]
fn beyond_the_cap_the_least_recently_read_go_first_but_none_read_in_the_last_two_days() {
    let now = SystemTime::now();
    let unit = |folder, days, bytes, removable| Unit {
        folder,
        paths: Vec::new(),
        read: days_ago(now, days),
        bytes,
        removable,
        orphaned: false,
    };
    let units = [
        unit(0, 0, 50, true),
        unit(0, 3, 30, true),
        unit(0, 5, 20, true),
        unit(0, 1, 40, true),
        unit(1, 6, 10, false),
        unit(2, 40, 5, true),
    ];
    let capped = Policy { cap: 90, ..UNCAPPED };
    assert_eq!(
        choose(&units, capped, now),
        [false, true, true, false, false, true],
        "still above the cap, the units read in the last two days and those a build holds stay"
    );
    assert_eq!(
        choose(&units, UNCAPPED, now),
        [false, false, false, false, false, true],
        "without a cap only the workspace no build read for a month goes"
    );
}

#[test]
fn a_profile_folder_a_build_holds_stays_untouched() {
    let now = SystemTime::now();
    let root = scratch("sweep-busy");
    let dest = folder(&root, "96aa06cc7aee6282").join("debug");
    let old = unit(&dest, "serde", "912dc2cb26f1601d", days_ago(now, 40), days_ago(now, 40));
    file(&dest.join(".cargo-build-lock"), now, now);
    settle(&root.join("96"), days_ago(now, 40));
    let held = File::options()
        .read(true)
        .write(true)
        .open(dest.join(".cargo-build-lock"))
        .unwrap();
    held.try_lock().unwrap();

    let report = sweep_under(&root, UNCAPPED, now, false);
    assert!(old.exists());
    assert_eq!(report.busy, 1);
    assert!(
        report
            .text(&root, UNCAPPED, false)
            .contains("1 build folder(s) in use left alone")
    );

    drop(held);
    let report = sweep_under(&root, UNCAPPED, now, false);
    assert!(!old.exists());
    assert_eq!(report.busy, 0);
}

#[test]
fn an_abandoned_build_folder_goes_whole_and_a_dry_run_removes_nothing() {
    let now = SystemTime::now();
    let root = scratch("sweep-abandoned");
    let folder = folder(&root, "2a269bb2bc7d94b8");
    let dest = folder.join("debug");
    let old = unit(
        &dest,
        "fractal",
        "912dc2cb26f1601d",
        days_ago(now, 40),
        days_ago(now, 40),
    );
    for path in [folder.join("CACHEDIR.TAG"), dest.join(".cargo-build-lock")] {
        file(&path, days_ago(now, 40), days_ago(now, 40));
    }
    settle(&root.join("2a"), days_ago(now, 40));

    let dry = sweep_under(&root, UNCAPPED, now, true);
    assert!(old.exists());
    assert!(dry.text(&root, UNCAPPED, true).contains("would remove 1 units"));

    let report = sweep_under(&root, UNCAPPED, now, false);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(
        !root.join("2a").exists(),
        "the build folder goes, and its shard with it"
    );
    assert!(root.exists());
}

#[test]
fn the_build_folder_of_a_worktree_that_is_gone_goes_whole_however_recently_read() {
    let now = SystemTime::now();
    let root = scratch("sweep-orphaned");
    let worktree = root.join("worktree");
    file(&worktree.join("Cargo.toml"), now, now);
    let live = folder(&root, "d5df618857e9c689");
    fs::write(live.join(crate::seed::WORKTREE), worktree.as_os_str().as_bytes()).unwrap();
    let used = unit(&live.join("debug"), "serde", "faba822365272399", now, now);
    let orphan = folder(&root, "9b0c3d64e1f2a7b8");
    fs::write(
        orphan.join(crate::seed::WORKTREE),
        root.join("removed").as_os_str().as_bytes(),
    )
    .unwrap();
    unit(&orphan.join("debug"), "serde", "faba822365272399", now, now);

    let report = sweep_under(&root, UNCAPPED, now, false);

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(used.exists(), "its worktree is still there");
    assert!(!root.join("9b").exists(), "the folder goes, and its shard with it");
}

#[test]
fn hashes_and_the_shared_root_come_from_the_names_cargo_gives() {
    assert_eq!(hash_in("libserde-515fb478708f4d1c.rlib"), Some("515fb478708f4d1c"));
    assert_eq!(hash_in("term_decoder-076acb337e59fa54"), Some("076acb337e59fa54"));
    assert_eq!(hash_in("my-bin-0123456789abcdef.d"), Some("0123456789abcdef"));
    assert_eq!(hash_in("serde.d"), None);
    assert_eq!(hash_in("serde-0123456789abcdeg"), None);
    assert_eq!(hash_in("serde-0123456789abcdef0"), None);
    assert_eq!(
        shared_root(Path::new("/Volumes/Scratch/cargo-build/ef/116096e23958fb")),
        Some(PathBuf::from("/Volumes/Scratch/cargo-build"))
    );
    assert_eq!(shared_root(Path::new("/Users/me/tapa/target")), None);
    assert_eq!(shared_root(Path::new("/x/ef/116096e23958fb-coverage")), None);
}
