//! Fills the build folder of a new worktree with the dependencies the main checkout has built, as APFS clones that take
//! no space until either side writes them, so the worktree builds only its own crates. git runs it for every
//! `git worktree add` through the post-checkout hook in `xtask/hooks`; Claude Code, which creates its worktrees without
//! git's hooks, through its PreToolUse hook before the first command in one. Taken over from Dango's
//! `xtask/src/seed.rs` with the parts of its `xtask/src/sweep.rs` it uses, at Dango's commit c59fb253.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::{Shell, System, files};

/// The file in a filled build folder that names its worktree; Dango's sweep reads it to remove the folder with the
/// worktree, so the name stays Dango's.
pub(crate) const WORKTREE: &str = ".xtask-worktree";

/// The file in a worktree's git folder that says it was filled, whatever came of it.
const SEEDED: &str = "xtask-seeded";

/// How long before the newest read in the main checkout's folder a unit may have been read and still be cloned.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How often `lock` tries before it takes a folder as held by a build.
const LOCK_TRIES: u32 = 10;

/// `cargo xtask seed <worktree>`, or `seed --hook` with the input of a Claude Code hook.
pub(crate) fn seed(args: &[String]) -> Result<(), String> {
    match args {
        [flag] if flag == "--hook" => from_hook(),
        [worktree] if !worktree.starts_with("--") => seed_worktree(Path::new(worktree)),
        _ => Err("usage: cargo xtask seed <worktree>|--hook".into()),
    }
}

/// Fills the worktree the hook's command runs in, once.
fn from_hook() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("hook input: {e}"))?;
    let hook: serde_json::Value = serde_json::from_str(&input).map_err(|e| format!("hook input: {e}"))?;
    let Some((worktree, git_dir)) = hook["cwd"]
        .as_str()
        .and_then(|cwd| linked_worktree(Path::new(cwd), &crate::root()))
    else {
        return Ok(());
    };
    if git_dir.join(SEEDED).exists() {
        return Ok(());
    }
    seed_worktree(&worktree)
}

fn seed_worktree(worktree: &Path) -> Result<(), String> {
    let main = crate::root();
    let Some((worktree, git_dir)) = linked_worktree(worktree, &main) else {
        return Err(format!("{} is no worktree of {}", worktree.display(), main.display()));
    };
    files::write(git_dir.join(SEEDED), "")?;
    let from = build_dir(&main)?;
    let to = build_dir(&worktree)?;
    if from == to || shared_root(&to).is_none() {
        println!("seed: {} has no build folder of its own", worktree.display());
        return Ok(());
    }
    match fill(&from, &to, &worktree, &registry_packages()?)? {
        Some(units) => println!("seed: cloned {units} dependency units into {}", to.display()),
        None => println!(
            "seed: a build holds {}; the worktree builds its dependencies itself",
            from.display()
        ),
    }
    Ok(())
}

/// The root and git folder of the worktree of the checkout `main` that `dir` lies in, unless that is `main` itself.
fn linked_worktree(dir: &Path, main: &Path) -> Option<(PathBuf, PathBuf)> {
    let dir = dir.canonicalize().ok()?;
    let root = dir.ancestors().find(|d| d.join(".git").exists())?;
    let link = fs::read_to_string(root.join(".git")).ok()?;
    let git_dir = root.join(link.strip_prefix("gitdir:")?.trim()).canonicalize().ok()?;
    let worktrees = main.join(".git/worktrees").canonicalize().ok()?;
    git_dir.starts_with(worktrees).then(|| (root.to_path_buf(), git_dir))
}

/// The packages of the main checkout's builds that come from a registry, not from a folder.
fn registry_packages() -> Result<HashSet<String>, String> {
    let shown =
        System.stdout(
            cargo()
                .current_dir(crate::root())
                .args(["metadata", "--format-version", "1", "--offline"]),
        )?;
    let metadata: serde_json::Value = serde_json::from_str(&shown).map_err(|e| format!("cargo metadata: {e}"))?;
    Ok(metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|package| !package["source"].is_null())
        .filter_map(|package| package["name"].as_str().map(str::to_owned))
        .collect())
}

/// The build folder of the workspace in `dir`, as cargo reports it.
fn build_dir(dir: &Path) -> Result<PathBuf, String> {
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
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// The folder that holds every workspace's build folder when `build-dir` names them by `{workspace-path-hash}`, which
/// cargo writes as two hex digits, a slash and fourteen more.
fn shared_root(build_dir: &Path) -> Option<PathBuf> {
    let shard = build_dir.parent()?;
    if is_hex(name(build_dir), 14) && is_hex(name(shard), 2) {
        shard.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// Clones the recent debug units of `packages` from the build folder `from` into `to`, which it marks as `worktree`'s,
/// and says how many; `None` while a build holds either folder.
pub(crate) fn fill(
    from: &Path,
    to: &Path,
    worktree: &Path,
    packages: &HashSet<String>,
) -> Result<Option<usize>, String> {
    let source = from.join("debug");
    if !source.join("build").is_dir() {
        return Ok(Some(0));
    }
    let dest = to.join("debug");
    files::create_dir_all(&dest)?;
    let tag = to.join("CACHEDIR.TAG");
    if !tag.exists() {
        files::copy(from.join("CACHEDIR.TAG"), &tag)?;
    }
    files::write(to.join(WORKTREE), worktree.as_os_str().as_bytes())?;
    let (Some(_reading), Some(_writing)) = (lock(&source), lock(&dest)) else {
        return Ok(None);
    };
    let mut cloned = 0;
    for (package, units) in recent_units(&source.join("build"), packages) {
        let into = dest.join("build").join(package);
        for unit in units {
            let clone = into.join(name(&unit));
            if clone.exists() {
                continue;
            }
            if let Err(e) = clone_dir(&unit, &clone) {
                let _ = fs::remove_dir_all(&clone);
                return Err(e);
            }
            cloned += 1;
        }
    }
    Ok(Some(cloned))
}

/// Copies the folder `from` to `to`; on APFS `fs::copy` clones each file, which keeps the times cargo compares.
fn clone_dir(from: &Path, to: &Path) -> Result<(), String> {
    files::create_dir_all(to)?;
    for entry in files::read_dir(from)? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let (source, dest) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", source.display()))?;
        if kind.is_dir() {
            clone_dir(&source, &dest)?;
        } else if kind.is_symlink() {
            let target = fs::read_link(&source).map_err(|e| format!("{}: {e}", source.display()))?;
            symlink(target, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        } else {
            files::copy(&source, &dest)?;
        }
    }
    Ok(())
}

/// The units in `build` of `packages`, by package, read within `MAX_AGE` of the newest read there; older ones are of an
/// earlier compiler or dependency version.
fn recent_units(build: &Path, packages: &HashSet<String>) -> BTreeMap<String, Vec<PathBuf>> {
    let read = |unit: &Path| last_read(&unit.join("fingerprint")).or_else(|| last_read(unit));
    let units: Vec<(String, SystemTime, PathBuf)> = subdirs(build)
        .into_iter()
        .flat_map(|package| {
            let name = name(&package).to_owned();
            subdirs(&package).into_iter().map(move |unit| (name.clone(), unit))
        })
        .filter(|(_, unit)| is_hex(name(unit), 16))
        .filter_map(|(name, unit)| Some((name, read(&unit)?, unit)))
        .collect();
    let Some(newest) = units.iter().map(|(_, read, _)| *read).max() else {
        return BTreeMap::new();
    };
    let mut found: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for (name, read, unit) in units {
        if packages.contains(&name) && newest.duration_since(read).unwrap_or_default() <= MAX_AGE {
            found.entry(name).or_default().push(unit);
        }
    }
    found
}

/// The lock cargo holds on a profile's folder while it builds there, unless a build holds it now.
fn lock(dest: &Path) -> Option<File> {
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
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

/// When a build last read `path` or, for a folder, anything directly in it. macOS refreshes a file's access time once
/// it is a day old, and cargo reads the fingerprint of every unit a build passes over. A folder counts by its
/// modification only, since listing it refreshes its access time.
fn last_read(path: &Path) -> Option<SystemTime> {
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

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
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

#[cfg(test)]
#[path = "seed_tests.rs"]
mod tests;
