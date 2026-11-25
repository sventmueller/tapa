//! Keeps the build folders bounded that cargo's `build-dir` shares between all workspaces of this Mac. Cargo never
//! removes a unit it no longer builds, such as one of an earlier compiler, dependency or feature set, or of a deleted
//! worktree, so the folders grow with every such change until something removes them.
//! Taken over from Dango's `xtask/src/sweep.rs` at commit e9710234; either one sweeps every workspace's folder.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::{Shell, System, files, flags};

const DAY: u64 = 24 * 60 * 60;

/// Which units a sweep removes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// A unit goes when the builds of its workspace have not read it for longer than this.
    pub(crate) max_age: Duration,
    /// All units of a workspace go when no build has read any of them for longer than this.
    pub(crate) abandoned: Duration,
    /// A unit read more recently than this stays in any case.
    pub(crate) keep: Duration,
    /// The bytes all units may take before the least recently read ones go.
    pub(crate) cap: u64,
}

/// The policy of `cargo xtask sweep`.
pub(crate) const POLICY: Policy = Policy {
    max_age: Duration::from_secs(7 * DAY),
    abandoned: Duration::from_secs(30 * DAY),
    // A unit's last read is known only to within a day (see `last_read`).
    keep: Duration::from_secs(2 * DAY),
    cap: 80_000_000_000,
};

/// What a sweep removed and left.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Each build folder with removals: its path below the root, the units and their bytes.
    pub(crate) folders: Vec<(String, usize, u64)>,
    /// The bytes of the units that stay.
    pub(crate) left: u64,
    /// The build folders a build held, whose units all stayed.
    pub(crate) busy: usize,
    /// What could not be removed, and why.
    pub(crate) failures: Vec<String>,
}

/// What cargo builds under one hash: in cargo 1.100's layout a folder, before it a fingerprint with the unit's files
/// in `deps/` and its build script's folder. An incremental cache counts as a unit of its own.
#[derive(Clone, Debug)]
struct Unit {
    folder: usize,
    paths: Vec<PathBuf>,
    read: SystemTime,
    bytes: u64,
    removable: bool,
    orphaned: bool,
}

impl Unit {
    fn of(paths: Vec<PathBuf>, read: Option<SystemTime>) -> Option<Unit> {
        let bytes = paths.iter().map(PathBuf::as_path).map(disk_usage).sum();
        Some(Unit {
            folder: 0,
            paths,
            read: read?,
            bytes,
            removable: false,
            orphaned: false,
        })
    }
}

/// `cargo xtask sweep [--dry-run]`.
pub(crate) fn sweep(args: &[String]) -> Result<(), String> {
    let [dry_run] = flags(args, ["--dry-run"])?;
    let Some(root) = shared_root(&build_dir(&crate::root())?) else {
        println!("sweep: cargo's build-dir is not shared between workspaces; nothing to do");
        return Ok(());
    };
    let report = sweep_under(&root, POLICY, SystemTime::now(), dry_run);
    print!("{}", report.text(&root, POLICY, dry_run));
    Ok(())
}

/// The build folder of the workspace in `dir`, as cargo reports it.
pub(crate) fn build_dir(dir: &Path) -> Result<PathBuf, String> {
    let shown = System.stdout(cargo().current_dir(dir).args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
        "--offline",
    ]))?;
    let metadata: serde_json::Value = serde_json::from_str(&shown).map_err(|e| format!("cargo metadata: {e}"))?;
    metadata["build_directory"]
        .as_str()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata names no build_directory".into())
}

/// The cargo that runs this task, or the one on the PATH.
pub(crate) fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// The folder that holds every workspace's build folder when `build-dir` names them by `{workspace-path-hash}`, which
/// cargo writes as two hex digits, a slash and fourteen more.
pub(crate) fn shared_root(build_dir: &Path) -> Option<PathBuf> {
    let shard = build_dir.parent()?;
    if is_hex(name(build_dir), 14) && is_hex(name(shard), 2) {
        shard.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// Sweeps every build folder under `root` with `policy` at `now`; a dry run only reports.
pub(crate) fn sweep_under(root: &Path, policy: Policy, now: SystemTime, dry_run: bool) -> Report {
    let folders = build_folders(root);
    let orphaned: Vec<bool> = folders.iter().map(|folder| worktree_gone(folder)).collect();
    let mut busy = vec![false; folders.len()];
    let mut locks = Vec::new();
    let mut units = Vec::new();
    for (folder, path) in folders.iter().enumerate() {
        for dest in dest_dirs(path) {
            let lock = lock(&dest);
            let removable = lock.is_some();
            busy[folder] |= !removable;
            units.extend(units_of(&dest).into_iter().map(|unit| Unit {
                folder,
                removable,
                orphaned: orphaned[folder],
                ..unit
            }));
            locks.extend(lock);
        }
    }
    let gone = choose(&units, policy, now);
    let mut report = Report {
        busy: busy.iter().filter(|b| **b).count(),
        ..Report::default()
    };
    let mut removed = vec![(0, 0); folders.len()];
    let mut kept = vec![0; folders.len()];
    for (unit, gone) in units.iter().zip(gone) {
        if gone {
            removed[unit.folder].0 += 1;
            removed[unit.folder].1 += unit.bytes;
            if !dry_run {
                report.failures.extend(remove(unit).err());
            }
        } else {
            kept[unit.folder] += 1;
            report.left += unit.bytes;
        }
    }
    for (folder, path) in folders.iter().enumerate() {
        let (count, bytes) = removed[folder];
        if count > 0 {
            report.folders.push((
                path.strip_prefix(root).unwrap_or(path).display().to_string(),
                count,
                bytes,
            ));
        }
        let idle = last_read(path).is_some_and(|read| age(now, read) > policy.keep);
        if !dry_run && kept[folder] == 0 && !busy[folder] && (idle || orphaned[folder]) {
            report.failures.extend(files::remove_dir_all(path).err());
        }
    }
    if !dry_run {
        for shard in subdirs(root) {
            // Only an empty shard goes.
            let _ = fs::remove_dir(shard);
        }
    }
    drop(locks);
    report
}

/// Which of `units` go: those of a worktree that is gone, then, unless read within `keep`, those the builds of their
/// workspace have not read for `max_age`, all of a workspace no build has read for `abandoned`, and the least recently
/// read while the rest is larger than `cap`.
fn choose(units: &[Unit], policy: Policy, now: SystemTime) -> Vec<bool> {
    let mut newest: HashMap<usize, SystemTime> = HashMap::new();
    for unit in units {
        let read = newest.entry(unit.folder).or_insert(unit.read);
        *read = (*read).max(unit.read);
    }
    let stale = |unit: &Unit| {
        let folder = newest[&unit.folder];
        age(folder, unit.read) > policy.max_age || age(now, folder) > policy.abandoned
    };
    let mut gone: Vec<bool> = units
        .iter()
        .map(|unit| unit.removable && (unit.orphaned || (age(now, unit.read) > policy.keep && stale(unit))))
        .collect();
    let mut size: u64 = units
        .iter()
        .zip(&gone)
        .filter(|(_, g)| !**g)
        .map(|(u, _)| u.bytes)
        .sum();
    let mut oldest: Vec<usize> = (0..units.len())
        .filter(|&i| !gone[i] && units[i].removable && age(now, units[i].read) > policy.keep)
        .collect();
    oldest.sort_by_key(|&i| units[i].read);
    for i in oldest {
        if size <= policy.cap {
            break;
        }
        gone[i] = true;
        size -= units[i].bytes;
    }
    gone
}

pub(crate) fn age(now: SystemTime, then: SystemTime) -> Duration {
    now.duration_since(then).unwrap_or_default()
}

/// Whether `folder` was filled for a worktree (`cargo xtask seed`) that no longer exists.
fn worktree_gone(folder: &Path) -> bool {
    fs::read(folder.join(crate::seed::WORKTREE))
        .is_ok_and(|path| !Path::new(OsStr::from_bytes(&path)).join("Cargo.toml").exists())
}

/// The build folders under `root`: `<2 hex digits>/<the rest of the hash>`, some with a suffix such as the coverage
/// build's, each marked by the `CACHEDIR.TAG` cargo writes into it.
fn build_folders(root: &Path) -> Vec<PathBuf> {
    subdirs(root)
        .into_iter()
        .filter(|shard| is_hex(name(shard), 2))
        .flat_map(|shard| subdirs(&shard))
        .filter(|folder| folder.join("CACHEDIR.TAG").is_file())
        .collect()
}

/// The folders of a build folder that cargo builds one profile into: `debug/`, `release/`, and the same under a
/// target such as `x86_64-apple-darwin/`.
fn dest_dirs(folder: &Path) -> Vec<PathBuf> {
    let is_dest = |dir: &PathBuf| {
        ["build", "deps", ".fingerprint", "incremental"]
            .iter()
            .any(|d| dir.join(d).is_dir())
    };
    let mut found = Vec::new();
    for dir in subdirs(folder) {
        if is_dest(&dir) {
            found.push(dir);
        } else {
            found.extend(subdirs(&dir).into_iter().filter(is_dest));
        }
    }
    found
}

/// The lock cargo holds on a profile's folder while it builds there, unless a build holds it now.
pub(crate) fn lock(dest: &Path) -> Option<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dest.join(".cargo-build-lock"))
        .ok()?;
    // A process forked meanwhile holds a copy of a just released lock until it execs, a build holds it for good.
    for _ in 0..LOCK_TRIES {
        if file.try_lock().is_ok() {
            return Some(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    None
}

/// How often `lock` tries before it takes a folder as held by a build.
const LOCK_TRIES: u32 = 10;

/// The units of a profile's folder in either layout, and its incremental caches.
fn units_of(dest: &Path) -> Vec<Unit> {
    let build = dest.join("build");
    let fingerprints = dest.join(".fingerprint");
    let mut deps = by_hash(entries(&dest.join("deps")));
    let mut found = Vec::new();
    for fingerprint in subdirs(&fingerprints) {
        let Some(hash) = hash_in(name(&fingerprint)) else {
            continue;
        };
        let mut paths = deps.remove(hash).unwrap_or_default();
        paths.extend(Some(build.join(name(&fingerprint))).filter(|dir| dir.exists()));
        let read = last_read(&fingerprint);
        paths.push(fingerprint);
        found.extend(Unit::of(paths, read));
    }
    for package in subdirs(&build) {
        if hash_in(name(&package)).is_some() {
            if !fingerprints.join(name(&package)).exists() {
                let read = last_read(&package);
                found.extend(Unit::of(vec![package], read));
            }
            continue;
        }
        for unit in subdirs(&package).into_iter().filter(|dir| is_hex(name(dir), 16)) {
            let read = last_read(&unit.join("fingerprint")).or_else(|| last_read(&unit));
            found.extend(Unit::of(vec![unit], read));
        }
    }
    for paths in deps.into_values() {
        let read = paths.iter().filter_map(|path| last_read(path)).max();
        found.extend(Unit::of(paths, read));
    }
    for cache in subdirs(&dest.join("incremental")) {
        let read = last_read(&cache);
        found.extend(Unit::of(vec![cache], read));
    }
    found
}

/// Removes a unit; a package's folder in cargo 1.100's layout goes with its last unit.
fn remove(unit: &Unit) -> Result<(), String> {
    for path in &unit.paths {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => files::remove_dir_all(path)?,
            Ok(_) => files::remove_file(path)?,
            Err(_) => continue,
        }
        if is_hex(name(path), 16)
            && let Some(package) = path.parent()
        {
            // Only a package without units goes.
            let _ = fs::remove_dir(package);
        }
    }
    Ok(())
}

/// When a build last read `path`, or a file in the folder `path`, to within a day: macOS refreshes a file's access
/// time once it is a day old, and cargo reads the fingerprint of every unit a build passes over. A folder counts by
/// its modification only, since listing it, as the sweep does, refreshes its access time.
pub(crate) fn last_read(path: &Path) -> Option<SystemTime> {
    let meta = fs::symlink_metadata(path).ok()?;
    let own = touched(&meta)?;
    if !meta.is_dir() {
        return Some(own);
    }
    let children = fs::read_dir(path)
        .ok()?
        .filter_map(|entry| entry.ok()?.metadata().ok())
        .filter_map(|meta| touched(&meta));
    Some(children.fold(own, SystemTime::max))
}

fn touched(meta: &fs::Metadata) -> Option<SystemTime> {
    let modified = meta.modified().ok()?;
    if meta.is_file() {
        Some(meta.accessed().map_or(modified, |read| read.max(modified)))
    } else {
        Some(modified)
    }
}

/// The bytes `path` takes on disk, with everything in it.
fn disk_usage(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    let own = meta.blocks() * 512;
    if meta.is_dir() {
        own + entries(path).iter().map(PathBuf::as_path).map(disk_usage).sum::<u64>()
    } else {
        own
    }
}

/// The files of `deps/` by the hash of the unit they belong to.
fn by_hash(paths: Vec<PathBuf>) -> HashMap<String, Vec<PathBuf>> {
    let mut found: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for path in paths {
        if let Some(hash) = hash_in(name(&path)).map(str::to_owned) {
            found.entry(hash).or_default().push(path);
        }
    }
    found
}

/// The hash cargo puts into the names of a unit's files and folders before cargo 1.100: a dash and 16 hex digits, at
/// the end or before an extension.
fn hash_in(name: &str) -> Option<&str> {
    name.match_indices('-').rev().find_map(|(at, _)| {
        let hash = name.get(at + 1..at + 17)?;
        let rest = &name[at + 17..];
        (is_hex(hash, 16) && (rest.is_empty() || rest.starts_with('.'))).then_some(hash)
    })
}

pub(crate) fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    found.sort();
    found
}

pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

impl Report {
    fn text(&self, root: &Path, policy: Policy, dry_run: bool) -> String {
        let mut text = String::new();
        for (folder, units, bytes) in &self.folders {
            let _ = writeln!(text, "  {folder}: {units} units, {}", gb(*bytes));
        }
        for failure in &self.failures {
            let _ = writeln!(text, "  {failure}");
        }
        let units: usize = self.folders.iter().map(|(_, units, _)| units).sum();
        let bytes: u64 = self.folders.iter().map(|(_, _, bytes)| bytes).sum();
        let verb = if dry_run { "would remove" } else { "removed" };
        let _ = write!(
            text,
            "sweep: {verb} {units} units ({}) under {}; {} left, cap {}",
            gb(bytes),
            root.display(),
            gb(self.left),
            gb(policy.cap)
        );
        if self.busy > 0 {
            let _ = write!(text, "; {} build folder(s) in use left alone", self.busy);
        }
        text.push('\n');
        text
    }
}

#[cfg(test)]
#[path = "sweep_tests.rs"]
pub(crate) mod tests;
