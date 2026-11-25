//! The product's one version: `version` in `[workspace.package]` of the root `Cargo.toml`, which every package of
//! the workspace takes with `version.workspace = true`, and those packages' entries in `Cargo.lock`. Only deploy
//! raises it.

use std::collections::BTreeSet;

use crate::source::Git;
use crate::*;

pub(crate) const MANIFEST: &str = "Cargo.toml";
pub(crate) const LOCK: &str = "Cargo.lock";

/// The files a raised version changes, in the order `git diff --name-only` prints them.
pub(crate) const FILES: [&str; 2] = [LOCK, MANIFEST];

/// What the fingerprint writes in place of the version, so that raising it changes no content.
const NEUTRAL: &str = "0.0.0";

const WORKSPACE_PACKAGE: &str = "[workspace.package]";

/// The version in `[workspace.package]` of `manifest`.
pub(crate) fn of_manifest(manifest: &str) -> Option<&str> {
    let mut section = "";
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
        } else if section == WORKSPACE_PACKAGE
            && let Some(version) = quoted(line, "version")
        {
            return Some(version);
        }
    }
    None
}

/// The name of the package `manifest` describes, if it takes the workspace's version.
pub(crate) fn member(manifest: &str) -> Option<&str> {
    let mut section = "";
    let (mut name, mut inherits) = (None, false);
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = line;
        } else if section == "[package]" {
            name = name.or_else(|| quoted(line, "name"));
            inherits |= line.replace(' ', "") == "version.workspace=true";
        }
    }
    name.filter(|_| inherits)
}

/// The value of `key = "value"`.
fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?
        .trim_start()
        .strip_prefix('=')?
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')
}

/// `manifest` with `version` in `[workspace.package]`.
pub(crate) fn manifest_with(manifest: &str, version: &str) -> Result<String, String> {
    let mut section = String::new();
    let mut found = false;
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.to_string();
        } else if section == WORKSPACE_PACKAGE && !found && quoted(trimmed, "version").is_some() {
            found = true;
            let end = if line.ends_with('\n') { "\n" } else { "" };
            out.push_str(&format!("version = \"{version}\"{end}"));
            continue;
        }
        out.push_str(line);
    }
    if found {
        Ok(out)
    } else {
        Err(format!("{MANIFEST} has no version in {WORKSPACE_PACKAGE}"))
    }
}

/// `lock` with `version` for each package of `members`.
pub(crate) fn lock_with(lock: &str, members: &BTreeSet<String>, version: &str) -> String {
    let mut current: Option<&str> = None;
    let mut out = String::with_capacity(lock.len());
    for line in lock.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            current = None;
        } else if let Some(name) = quoted(trimmed, "name") {
            current = Some(name);
        } else if current.is_some_and(|name| members.contains(name)) && quoted(trimmed, "version").is_some() {
            let end = if line.ends_with('\n') { "\n" } else { "" };
            out.push_str(&format!("version = \"{version}\"{end}"));
            current = None;
            continue;
        }
        out.push_str(line);
    }
    out
}

/// The packages among `manifests` that take the workspace's version.
pub(crate) fn members<'a>(manifests: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    manifests.into_iter().filter_map(member).map(str::to_string).collect()
}

/// `text` of the file at `path` with the version replaced by a placeholder, if it is one that carries the version.
pub(crate) fn neutral(path: &str, text: &str, members: &BTreeSet<String>) -> Result<Option<String>, String> {
    match path {
        MANIFEST => manifest_with(text, NEUTRAL).map(Some),
        LOCK => Ok(Some(lock_with(text, members, NEUTRAL))),
        _ => Ok(None),
    }
}

/// The package manifests among the paths `git ls-files` printed.
pub(crate) fn manifest_paths(listed: &str) -> Vec<&str> {
    listed
        .lines()
        .filter(|path| *path == MANIFEST || path.ends_with("/Cargo.toml"))
        .collect()
}

/// Writes `version` into the working tree's `Cargo.toml` and `Cargo.lock`.
pub(crate) fn write(sh: &mut dyn Shell, git: &Git, version: &str) -> Result<(), String> {
    if !is_version(version) {
        return Err(format!("{version:?} is not major.minor.build"));
    }
    let listed = git.output(sh, &["ls-files", "--", MANIFEST, "*/Cargo.toml"])?;
    let mut manifests = Vec::new();
    for path in manifest_paths(&listed) {
        manifests.push(files::read_to_string(git.dir().join(path))?);
    }
    let members = members(manifests.iter().map(String::as_str));
    let manifest = git.dir().join(MANIFEST);
    let raised = manifest_with(&files::read_to_string(&manifest)?, version)?;
    files::write(&manifest, raised)?;
    let lock = git.dir().join(LOCK);
    if lock.exists() {
        let raised = lock_with(&files::read_to_string(&lock)?, &members, version);
        files::write(&lock, raised)?;
    }
    Ok(())
}

/// Puts the committed `Cargo.toml` and `Cargo.lock` back into the working tree and the index.
pub(crate) fn restore(sh: &mut dyn Shell, git: &Git) -> Result<(), String> {
    git.output(
        sh,
        &[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            MANIFEST,
            LOCK,
        ],
    )
    .map(|_| ())
}

#[cfg(test)]
#[path = "version_tests.rs"]
pub(crate) mod tests;
