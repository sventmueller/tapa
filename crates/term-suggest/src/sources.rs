//! The dynamic sources of the delayed stage (E8, R10): which source an argument wants, from the spec's generator
//! names and our own rules for the most used CLIs, and what it finds. Project files are read, never run (E16); the
//! programs a source asks, such as `docker` and `kubectl`, come from the user's `PATH` outside the project and are
//! given up after the deadline.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::complete::{Candidate, Kind, Wanted};
use crate::files;

/// What a dynamic source looks up.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    GitBranches,
    GitRemotes,
    PackageScripts,
    MakeTargets,
    JustRecipes,
    CargoPackages,
    CargoTargets(&'static str),
    SshHosts,
    /// Containers of `docker` or `podman`, running ones or all.
    Containers(&'static str, bool),
    Images(&'static str),
    KubernetesKinds,
    KubernetesNames(String),
}

const GIT_BRANCH_COMMANDS: &[&str] = &[
    "checkout",
    "switch",
    "merge",
    "rebase",
    "branch",
    "cherry-pick",
    "log",
    "diff",
    "reset",
    "show",
];
const CONTAINER_COMMANDS: &[&str] = &[
    "exec", "logs", "stop", "start", "restart", "rm", "attach", "inspect", "kill", "pause", "unpause", "top", "port",
];
const KUBECTL_VERBS: &[&str] = &[
    "get",
    "describe",
    "delete",
    "edit",
    "logs",
    "exec",
    "port-forward",
    "rollout",
];

/// Whether one of our own rules gives the command whose words before the one being typed are `words` a source.
pub(crate) fn has_rule<S: AsRef<str>>(words: &[S]) -> bool {
    rule(&words.iter().map(|w| w.as_ref()).collect::<Vec<_>>()).is_some()
}

/// The source for what `wanted` describes: the spec's generator names first, then our own rules.
pub(crate) fn source_for(wanted: &Wanted) -> Option<Source> {
    let words: Vec<&str> = wanted.words.iter().map(String::as_str).collect();
    let program = *words.first()?;
    for marker in &wanted.markers {
        if let Some(source) = from_marker(program, marker) {
            return Some(source);
        }
    }
    rule(&words)
}

fn from_marker(program: &str, marker: &str) -> Option<Source> {
    let lower = marker.to_lowercase();
    Some(match () {
        () if program == "git" && lower.contains("remote") => Source::GitRemotes,
        () if program == "git" && (lower.contains("branch") || lower.contains("treeish")) => Source::GitBranches,
        () if lower.contains("package.json") => Source::PackageScripts,
        () if lower.contains("listtargets") => Source::MakeTargets,
        () if lower.contains("knownhosts") || lower.contains("confighosts") => Source::SshHosts,
        () if lower.contains("targetgenerator") && lower.contains("example") => Source::CargoTargets("example"),
        () if lower.contains("targetgenerator") => Source::CargoTargets("bin"),
        () if lower.contains(" ps") && matches!(program, "docker" | "podman") => {
            Source::Containers(engine(program), lower.contains("-a"))
        }
        () if lower.contains("image") && matches!(program, "docker" | "podman") => Source::Images(engine(program)),
        () if lower.contains("api-resources") => Source::KubernetesKinds,
        () => return None,
    })
}

fn engine(program: &str) -> &'static str {
    if program == "podman" { "podman" } else { "docker" }
}

fn rule(words: &[&str]) -> Option<Source> {
    let (&program, rest) = words.split_first()?;
    let sub = rest.first().copied().unwrap_or_default();
    let last = words.last().copied().unwrap_or_default();
    Some(match program {
        "git" if matches!(sub, "push" | "pull" | "fetch") && rest.len() == 1 => Source::GitRemotes,
        "git" if matches!(sub, "push" | "pull") => Source::GitBranches,
        "git" if GIT_BRANCH_COMMANDS.contains(&sub) => Source::GitBranches,
        "npm" | "bun" | "pnpm" | "yarn" if sub == "run" && rest.len() == 1 => Source::PackageScripts,
        "make" | "gmake" if !last.starts_with('-') => Source::MakeTargets,
        "just" if !last.starts_with('-') => Source::JustRecipes,
        "cargo" if matches!(last, "-p" | "--package") => Source::CargoPackages,
        "cargo" if last == "--bin" => Source::CargoTargets("bin"),
        "cargo" if last == "--example" => Source::CargoTargets("example"),
        "ssh" | "scp" | "sftp" | "mosh" | "rsync" if !last.starts_with('-') => Source::SshHosts,
        "docker" | "podman" if CONTAINER_COMMANDS.contains(&sub) => Source::Containers(engine(program), sub == "rm"),
        "docker" | "podman" if matches!(sub, "rmi" | "run" | "create" | "pull" | "push" | "tag") => {
            Source::Images(engine(program))
        }
        "kubectl" if KUBECTL_VERBS.contains(&sub) => {
            let positional: Vec<&str> = rest[1..].iter().copied().filter(|w| !w.starts_with('-')).collect();
            match positional.first() {
                None if sub == "logs" || sub == "exec" || sub == "port-forward" => {
                    Source::KubernetesNames("pods".into())
                }
                None => Source::KubernetesKinds,
                Some(kind) if positional.len() == 1 => Source::KubernetesNames((*kind).to_string()),
                Some(_) => return None,
            }
        }
        _ => return None,
    })
}

impl Source {
    /// Whether the source reads files only, which are cheap, rather than asking a program.
    pub(crate) fn reads_files(&self) -> bool {
        !matches!(
            self,
            Source::Containers(..) | Source::Images(_) | Source::KubernetesKinds | Source::KubernetesNames(_)
        )
    }

    /// What the source finds for `folder`, at the latest by `deadline`.
    pub(crate) fn find(&self, folder: &Path, home: &Path, deadline: Instant) -> Vec<Candidate> {
        let found: Vec<(String, Option<String>)> = match self {
            Source::GitBranches => plain(files::git_branches(folder)),
            Source::GitRemotes => plain(files::git_remotes(folder)),
            Source::PackageScripts => files::package_scripts(folder),
            Source::MakeTargets => plain(files::make_targets(folder)),
            Source::JustRecipes => files::just_recipes(folder),
            Source::CargoPackages => plain(files::cargo_packages(folder)),
            Source::CargoTargets(kind) => plain(files::cargo_targets(folder, kind)),
            Source::SshHosts => plain(files::ssh_hosts(home)),
            Source::Containers(engine, all) => {
                let mut args = vec!["ps", "--format", "{{.Names}}"];
                if *all {
                    args.push("-a");
                }
                plain(ask(engine, &args, folder, home, deadline))
            }
            Source::Images(engine) => plain(
                ask(
                    engine,
                    &["images", "--format", "{{.Repository}}:{{.Tag}}"],
                    folder,
                    home,
                    deadline,
                )
                .into_iter()
                .filter(|image| !image.contains("<none>"))
                .collect(),
            ),
            Source::KubernetesKinds => plain(ask("kubectl", &["api-resources", "-o", "name"], folder, home, deadline)),
            Source::KubernetesNames(kind) => plain(
                ask("kubectl", &["get", kind, "-o", "name"], folder, home, deadline)
                    .into_iter()
                    .map(|name| name.rsplit('/').next().unwrap_or_default().to_string())
                    .collect(),
            ),
        };
        found
            .into_iter()
            .map(|(text, description)| Candidate {
                text,
                description,
                kind: Kind::Dynamic,
            })
            .collect()
    }
}

fn plain(names: Vec<String>) -> Vec<(String, Option<String>)> {
    names.into_iter().map(|name| (name, None)).collect()
}

/// The program `name` in the user's `PATH`, from absolute folders outside `folder` only (E16).
pub(crate) fn in_path(name: &str, folder: &Path) -> Option<PathBuf> {
    path_folders(folder)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|path| is_executable(path))
}

/// The folders of `PATH` and the usual ones for Homebrew, outside `folder` and absolute.
pub(crate) fn path_folders(folder: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = crate::programs::programs()
        .path()
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    dirs.extend(
        [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
        .map(PathBuf::from),
    );
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| dir.is_absolute() && !dir.starts_with(folder) && seen.insert(dir.clone()));
    dirs
}

pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// The lines `program args` prints, run in the home folder; nothing if it is not done by `deadline`.
fn ask(program: &str, args: &[&str], folder: &Path, home: &Path, deadline: Instant) -> Vec<String> {
    let Some(path) = in_path(program, folder) else {
        return Vec::new();
    };
    crate::programs::programs()
        .output(&path, args, home, deadline)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wanted(words: &[&str], markers: &[&str]) -> Option<Source> {
        source_for(&Wanted {
            words: words.iter().map(|w| w.to_string()).collect(),
            markers: markers.iter().map(|m| m.to_string()).collect(),
        })
    }

    #[test]
    fn markers_and_rules_pick_the_source() {
        let branches = "script:git --no-optional-locks branch --no-color --sort=-committerdate";
        assert_eq!(wanted(&["git", "checkout"], &[branches]), Some(Source::GitBranches));
        assert_eq!(wanted(&["git", "push"], &[]), Some(Source::GitRemotes));
        assert_eq!(wanted(&["git", "push", "origin"], &[]), Some(Source::GitBranches));
        assert_eq!(wanted(&["bun", "run"], &[]), Some(Source::PackageScripts));
        assert_eq!(wanted(&["make"], &["listTargets"]), Some(Source::MakeTargets));
        assert_eq!(wanted(&["ssh"], &["knownHosts", "configHosts"]), Some(Source::SshHosts));
        assert_eq!(
            wanted(&["docker", "exec"], &["script:docker ps --format {{ json . }}"]),
            Some(Source::Containers("docker", false))
        );
        assert_eq!(wanted(&["kubectl", "get"], &[]), Some(Source::KubernetesKinds));
        assert_eq!(
            wanted(&["kubectl", "get", "pods"], &[]),
            Some(Source::KubernetesNames("pods".into()))
        );
        assert_eq!(wanted(&["cargo", "test", "-p"], &[]), Some(Source::CargoPackages));
        assert_eq!(wanted(&["ls"], &[]), None);
    }

    #[test]
    fn programs_come_from_the_path_outside_the_project_and_are_given_up_late() {
        let dir = term_base::testing::scratch("suggest-sources");
        assert!(in_path("ls", &dir).is_some());
        assert!(path_folders(Path::new("/usr")).iter().all(|d| !d.starts_with("/usr")));
        let started = Instant::now();
        let lines = ask("sleep", &["5"], &dir, &dir, started + Duration::from_millis(100));
        assert!(lines.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2), "killed at the deadline");
        assert_eq!(ask("echo", &["a"], &dir, &dir, started + Duration::from_secs(5)), ["a"]);
    }
}
