//! What the dynamic sources read from files (E16): git's refs and config, `package.json`, Makefiles, justfiles,
//! Cargo manifests and the SSH configuration. Nothing here runs a program.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The most names a source gives.
const MOST: usize = 500;

/// `name` in `folder` or the nearest folder above it.
pub(crate) fn upwards(folder: &Path, names: &[&str]) -> Option<PathBuf> {
    folder
        .ancestors()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.exists())
}

/// The repository's shared git folder for `folder`: `.git`, a worktree's `commondir` followed.
fn git_dir(folder: &Path) -> Option<PathBuf> {
    let dot = upwards(folder, &[".git"])?;
    let dir = if dot.is_dir() {
        dot
    } else {
        let text = std::fs::read_to_string(&dot).ok()?;
        let target = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        dot.parent()?.join(target)
    };
    match std::fs::read_to_string(dir.join("commondir")) {
        Ok(common) => Some(dir.join(common.trim())),
        Err(_) => Some(dir),
    }
}

/// Local branches, most lately changed first, then remote ones without a local namesake.
pub(crate) fn git_branches(folder: &Path) -> Vec<String> {
    let Some(dir) = git_dir(folder) else {
        return Vec::new();
    };
    let mut local: Vec<(SystemTime, String)> = Vec::new();
    refs_under(&dir.join("refs/heads"), "", &mut local);
    let mut remote: Vec<(SystemTime, String)> = Vec::new();
    refs_under(&dir.join("refs/remotes"), "", &mut remote);
    if let Ok(packed) = std::fs::read_to_string(dir.join("packed-refs")) {
        for line in packed.lines() {
            let Some((_, name)) = line.split_once(' ') else {
                continue;
            };
            if let Some(branch) = name.strip_prefix("refs/heads/") {
                local.push((SystemTime::UNIX_EPOCH, branch.to_string()));
            } else if let Some(branch) = name.strip_prefix("refs/remotes/") {
                remote.push((SystemTime::UNIX_EPOCH, branch.to_string()));
            }
        }
    }
    local.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    remote.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let mut names: Vec<String> = Vec::new();
    for (_, name) in local {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for (_, name) in remote {
        let short = name.split_once('/').map_or(name.as_str(), |(_, rest)| rest);
        if short != "HEAD" && !names.iter().any(|n| n == short || *n == name) {
            names.push(name);
        }
    }
    names.truncate(MOST);
    names
}

fn refs_under(dir: &Path, prefix: &str, out: &mut Vec<(SystemTime, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            refs_under(&entry.path(), &format!("{name}/"), out);
        } else if out.len() < MOST * 4 {
            out.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), name));
        }
    }
}

/// The remotes named in the repository's config.
pub(crate) fn git_remotes(folder: &Path) -> Vec<String> {
    let Some(config) = git_dir(folder).and_then(|dir| std::fs::read_to_string(dir.join("config")).ok()) else {
        return Vec::new();
    };
    config
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("[remote \"")?
                .strip_suffix("\"]")
                .map(str::to_string)
        })
        .collect()
}

/// The scripts of the nearest `package.json`, each with its command as the description.
pub(crate) fn package_scripts(folder: &Path) -> Vec<(String, Option<String>)> {
    let Some(text) = upwards(folder, &["package.json"]).and_then(|path| std::fs::read_to_string(path).ok()) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    json.get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|scripts| {
            scripts
                .iter()
                .map(|(name, command)| (name.clone(), command.as_str().map(str::to_string)))
                .collect()
        })
        .unwrap_or_default()
}

/// The targets of the Makefile in `folder`, as `make` finds it.
pub(crate) fn make_targets(folder: &Path) -> Vec<String> {
    let Some(text) = ["GNUmakefile", "makefile", "Makefile"]
        .iter()
        .find_map(|name| std::fs::read_to_string(folder.join(name)).ok())
    else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        if rest.starts_with('=') || head.contains('=') || head.contains('$') || head.contains('%') {
            continue;
        }
        for target in head.split_whitespace() {
            if !targets.iter().any(|t| t == target) {
                targets.push(target.to_string());
            }
        }
    }
    targets.truncate(MOST);
    targets
}

/// The recipes of the nearest justfile, each with the comment above it as the description.
pub(crate) fn just_recipes(folder: &Path) -> Vec<(String, Option<String>)> {
    let Some(text) =
        upwards(folder, &["justfile", "Justfile", ".justfile"]).and_then(|path| std::fs::read_to_string(path).ok())
    else {
        return Vec::new();
    };
    let mut recipes = Vec::new();
    let mut comment: Option<String> = None;
    for line in text.lines() {
        if let Some(doc) = line.strip_prefix('#') {
            comment = Some(doc.trim().to_string());
            continue;
        }
        let name: String = line
            .trim_start_matches('@')
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        let is_recipe = !line.starts_with([' ', '\t'])
            && !name.is_empty()
            && !matches!(name.as_str(), "set" | "alias" | "export" | "import" | "mod")
            && line.split_once(':').is_some_and(|(_, rest)| !rest.starts_with('='));
        if is_recipe {
            recipes.push((name, comment.take()));
        } else if !line.trim().is_empty() {
            comment = None;
        }
    }
    recipes
}

/// The packages of the nearest Cargo workspace.
pub(crate) fn cargo_packages(folder: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for manifest in workspace_manifests(folder) {
        if let Some(name) = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|table| table.get("package")?.get("name")?.as_str().map(str::to_string))
        {
            names.push(name);
        }
    }
    names
}

/// The binaries (`kind` `bin`) or examples of the packages of the nearest Cargo workspace.
pub(crate) fn cargo_targets(folder: &Path, kind: &str) -> Vec<String> {
    let mut names = Vec::new();
    for manifest in workspace_manifests(folder) {
        let Some(dir) = manifest.parent() else { continue };
        let table = std::fs::read_to_string(&manifest)
            .ok()
            .and_then(|text| text.parse::<toml::Table>().ok())
            .unwrap_or_default();
        if let Some(targets) = table.get(kind).and_then(toml::Value::as_array) {
            names.extend(
                targets
                    .iter()
                    .filter_map(|t| t.get("name")?.as_str().map(str::to_string)),
            );
        }
        let folder = if kind == "bin" {
            dir.join("src/bin")
        } else {
            dir.join("examples")
        };
        if let Ok(entries) = std::fs::read_dir(folder) {
            names.extend(entries.flatten().filter_map(|entry| {
                let path = entry.path();
                let target = path.extension().is_some_and(|e| e == "rs") || path.join("main.rs").exists();
                target
                    .then(|| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
                    .flatten()
            }));
        }
        if kind == "bin"
            && dir.join("src/main.rs").exists()
            && let Some(name) = table.get("package").and_then(|p| p.get("name")?.as_str())
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The manifests of the nearest workspace: its root's and those of the members its globs of one `*` name.
fn workspace_manifests(folder: &Path) -> Vec<PathBuf> {
    let manifests: Vec<PathBuf> = folder
        .ancestors()
        .map(|dir| dir.join("Cargo.toml"))
        .filter(|path| path.is_file())
        .collect();
    let root = manifests
        .iter()
        .rev()
        .find(|path| std::fs::read_to_string(path).is_ok_and(|text| text.contains("[workspace]")))
        .or(manifests.first());
    let Some(root) = root.cloned() else {
        return Vec::new();
    };
    let table = std::fs::read_to_string(&root)
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok())
        .unwrap_or_default();
    let mut out = vec![root.clone()];
    let base = root.parent().map(Path::to_path_buf).unwrap_or_default();
    let members = table
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for member in members.iter().filter_map(toml::Value::as_str) {
        match member.strip_suffix("/*") {
            Some(parent) => {
                if let Ok(entries) = std::fs::read_dir(base.join(parent)) {
                    out.extend(
                        entries
                            .flatten()
                            .map(|e| e.path().join("Cargo.toml"))
                            .filter(|p| p.is_file()),
                    );
                }
            }
            None => out.push(base.join(member).join("Cargo.toml")),
        }
    }
    out.retain(|path| path.is_file());
    out.sort();
    out.dedup();
    out
}

/// The hosts of `~/.ssh/config` without wildcards, then those of `known_hosts` that are not hashed.
pub fn ssh_hosts(home: &Path) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    if let Ok(config) = std::fs::read_to_string(home.join(".ssh/config")) {
        for line in config.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("Host ").or_else(|| line.strip_prefix("host ")) else {
                continue;
            };
            hosts.extend(
                rest.split_whitespace()
                    .filter(|h| !h.contains(['*', '?', '!']))
                    .map(str::to_string),
            );
        }
    }
    if let Ok(known) = std::fs::read_to_string(home.join(".ssh/known_hosts")) {
        for line in known.lines() {
            let Some(field) = line.split_whitespace().next() else {
                continue;
            };
            if field.starts_with(['|', '@', '#']) {
                continue;
            }
            for host in field.split(',') {
                let host = host.trim_start_matches('[').split("]:").next().unwrap_or_default();
                if !host.is_empty() && !hosts.iter().any(|h| h == host) {
                    hosts.push(host.to_string());
                }
            }
        }
    }
    hosts.truncate(MOST);
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_branches_and_remotes_come_from_the_refs_and_the_config() {
        let repo = term_base::testing::scratch("suggest-files-git");
        std::fs::create_dir_all(repo.join(".git/refs/heads/feature")).unwrap();
        std::fs::create_dir_all(repo.join(".git/refs/remotes/origin")).unwrap();
        std::fs::write(repo.join(".git/refs/heads/main"), "x").unwrap();
        std::fs::write(repo.join(".git/refs/heads/feature/a"), "x").unwrap();
        std::fs::write(repo.join(".git/refs/remotes/origin/main"), "x").unwrap();
        std::fs::write(repo.join(".git/refs/remotes/origin/HEAD"), "x").unwrap();
        std::fs::write(
            repo.join(".git/packed-refs"),
            "# pack\nabc refs/heads/old\nabc refs/remotes/origin/gone\n",
        )
        .unwrap();
        std::fs::write(repo.join(".git/config"), "[core]\n[remote \"origin\"]\n\turl = x\n").unwrap();
        let mut branches = git_branches(&repo);
        branches.sort();
        assert_eq!(branches, ["feature/a", "main", "old", "origin/gone"]);
        assert_eq!(git_remotes(&repo), ["origin"]);
    }

    #[test]
    fn scripts_targets_recipes_and_hosts_are_read_from_their_files() {
        let dir = term_base::testing::scratch("suggest-files-project");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"dev":"vite","test":"vitest"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Makefile"),
            "VAR := 1\n.PHONY: all\nall: build\nbuild test:\n\tcc\n%.o: %.c\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("justfile"),
            "set shell := [\"zsh\"]\n# Runs it\nrun arg:\n  echo\n@quiet:\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".ssh")).unwrap();
        std::fs::write(dir.join(".ssh/config"), "Host evo-1 evo-2\nHost *\n").unwrap();
        std::fs::write(
            dir.join(".ssh/known_hosts"),
            "nas,10.0.0.2 ssh-ed25519 K\n|1|hash ssh-rsa K\n[h]:2222 x\n",
        )
        .unwrap();
        let scripts: Vec<String> = package_scripts(&dir).into_iter().map(|s| s.0).collect();
        assert_eq!(scripts, ["dev", "test"]);
        assert_eq!(make_targets(&dir), ["all", "build", "test"]);
        assert_eq!(
            just_recipes(&dir),
            [
                ("run".to_string(), Some("Runs it".to_string())),
                ("quiet".to_string(), None)
            ]
        );
        assert_eq!(ssh_hosts(&dir), ["evo-1", "evo-2", "nas", "10.0.0.2", "h"]);
    }

    #[test]
    fn cargo_packages_and_binaries_come_from_the_workspace() {
        let dir = term_base::testing::scratch("suggest-files-cargo");
        std::fs::create_dir_all(dir.join("crates/a/src/bin")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/*\"]\n").unwrap();
        std::fs::write(dir.join("crates/a/Cargo.toml"), "[package]\nname = \"alpha\"\n").unwrap();
        std::fs::write(dir.join("crates/a/src/main.rs"), "").unwrap();
        std::fs::write(dir.join("crates/a/src/bin/tool.rs"), "").unwrap();
        assert_eq!(cargo_packages(&dir.join("crates/a")), ["alpha"]);
        assert_eq!(cargo_targets(&dir, "bin"), ["alpha", "tool"]);
    }
}
