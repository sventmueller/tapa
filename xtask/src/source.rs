//! The source a deploy builds the release from: clean `main`, the clean and pushed siblings, and the fingerprint of the
//! content, the git tree without the version, which only deploy raises, and with the commits and changes of `../splat`
//! and `../lowspeak`. Dango's seal (`refs/ci-seals/…`) uses the same fingerprint; Tapa has no `make ci` and no seals.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::*;

/// The branch `deploy` pushes.
pub(crate) const BRANCH: &str = "main";

/// The checkouts beside the repository that the workspace builds crates from by path.
pub(crate) const SIBLINGS: [&str; 2] = ["splat", "lowspeak"];

/// The folder of the fingerprint's tree that records the siblings as gitlinks, which a tree holds without the
/// objects they name.
const SIBLINGS_DIR: &str = ".siblings";

/// A repository and the environment its git commands get.
pub(crate) struct Git {
    dir: PathBuf,
    env: Vec<(String, String)>,
}

impl Git {
    pub(crate) fn at(dir: &Path) -> Git {
        Git {
            dir: dir.to_path_buf(),
            env: Vec::new(),
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(&self.dir)
            .args(args)
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        cmd
    }

    pub(crate) fn output(&self, sh: &mut dyn Shell, args: &[&str]) -> Result<String, String> {
        sh.stdout(&mut self.command(args))
    }

    /// The checkout `name` beside this one, with the same environment.
    pub(crate) fn sibling(&self, name: &str) -> Git {
        Git {
            dir: self.dir.join("..").join(name),
            env: self.env.clone(),
        }
    }

    /// The object `args` name, such as `HEAD`.
    fn object(&self, sh: &mut dyn Shell, args: &[&str]) -> Result<String, String> {
        let name = self.output(sh, args)?;
        let name = name.trim();
        if is_object_name(name) {
            Ok(name.to_string())
        } else {
            Err(format!("git {} printed {name:?}, not an object name", args.join(" ")))
        }
    }
}

/// Which content a fingerprint describes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Content {
    /// HEAD with every change of the working tree, untracked files that are not ignored included.
    WorkingTree,
    /// HEAD's commit.
    Head,
}

/// The fingerprint of `content`: the name of its git tree without the version and with the state of the siblings,
/// written to the repository's objects.
pub(crate) fn fingerprint(sh: &mut dyn Shell, git: &Git, content: Content) -> Result<String, String> {
    let index = scratch_index();
    let tree = sibling_entries(sh, git).and_then(|siblings| tree_without_version(sh, git, content, &siblings, &index));
    let _ = std::fs::remove_file(&index);
    tree
}

/// A path for an index file of its own, which git creates on first use.
fn scratch_index() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "xtask-source-{}-{}.index",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `git` with the index file `index` in place of the repository's own.
fn indexed(sh: &mut dyn Shell, git: &Git, index: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git.command(args);
    cmd.env("GIT_INDEX_FILE", index);
    sh.stdout(&mut cmd)
}

fn tree_without_version(
    sh: &mut dyn Shell,
    git: &Git,
    content: Content,
    siblings: &[(String, String)],
    index: &Path,
) -> Result<String, String> {
    indexed(sh, git, index, &["read-tree", "HEAD"])?;
    if content == Content::WorkingTree {
        indexed(sh, git, index, &["add", "--all"])?;
    }
    without_version(sh, git, index)?;
    for (path, object) in siblings {
        let entry = format!("160000,{object},{path}");
        indexed(sh, git, index, &["update-index", "--add", "--cacheinfo", &entry])?;
    }
    let tree = indexed(sh, git, index, &["write-tree"])?;
    let tree = tree.trim();
    if is_object_name(tree) {
        Ok(tree.to_string())
    } else {
        Err(format!("git write-tree printed {tree:?}, not an object name"))
    }
}

/// Replaces the version in the index's `Cargo.toml` and `Cargo.lock` by a placeholder, writing their new contents to
/// the repository's objects.
fn without_version(sh: &mut dyn Shell, git: &Git, index: &Path) -> Result<(), String> {
    let listed = indexed(
        sh,
        git,
        index,
        &["ls-files", "--", version::MANIFEST, version::LOCK, "*/Cargo.toml"],
    )?;
    let mut manifests = Vec::new();
    for path in version::manifest_paths(&listed) {
        manifests.push(indexed(sh, git, index, &["show", &format!(":{path}")])?);
    }
    let members = version::members(manifests.iter().map(String::as_str));
    for path in version::FILES {
        if !listed.lines().any(|listed| listed == path) {
            continue;
        }
        let text = indexed(sh, git, index, &["show", &format!(":{path}")])?;
        let Some(neutral) = version::neutral(path, &text, &members)? else {
            continue;
        };
        let file = scratch_index();
        files::write(&file, neutral)?;
        let file_arg = file.display().to_string();
        let object = indexed(sh, git, index, &["hash-object", "-w", "--no-filters", &file_arg]);
        let _ = std::fs::remove_file(&file);
        let object = object?;
        let entry = format!("100644,{},{path}", object.trim());
        indexed(sh, git, index, &["update-index", "--cacheinfo", &entry])?;
    }
    Ok(())
}

/// What the fingerprint records of each sibling there is, as a path below `SIBLINGS_DIR` and the object it names: its
/// HEAD commit, and the tree of its working tree when that holds changes. Writing that tree stores the changed files
/// in the sibling's objects, as `git add` would, and leaves its index alone.
fn sibling_entries(sh: &mut dyn Shell, git: &Git) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    for name in SIBLINGS {
        let sibling = git.sibling(name);
        if !sibling.dir.exists() {
            continue;
        }
        let head = sibling.object(sh, &["rev-parse", "HEAD"])?;
        let committed = sibling.object(sh, &["rev-parse", "HEAD^{tree}"])?;
        let index = scratch_index();
        let working = indexed(sh, &sibling, &index, &["read-tree", "HEAD"])
            .and_then(|_| indexed(sh, &sibling, &index, &["add", "--all"]))
            .and_then(|_| indexed(sh, &sibling, &index, &["write-tree"]));
        let _ = std::fs::remove_file(&index);
        let working = working?.trim().to_string();
        if !is_object_name(&working) {
            return Err(format!(
                "git write-tree printed {working:?} in ../{name}, not an object name"
            ));
        }
        entries.push((format!("{SIBLINGS_DIR}/{name}"), head));
        if working != committed {
            entries.push((format!("{SIBLINGS_DIR}/{name}-changes"), working));
        }
    }
    Ok(entries)
}

/// A full SHA-1 or SHA-256 object name.
fn is_object_name(name: &str) -> bool {
    matches!(name.len(), 40 | 64) && name.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Refuses on another branch or with changes not committed, and returns HEAD's fingerprint.
pub(crate) fn ready(sh: &mut dyn Shell, git: &Git) -> Result<String, String> {
    let branch = git.output(sh, &["branch", "--show-current"])?;
    if branch.trim() != BRANCH {
        return Err(format!(
            "deploy pushes {BRANCH}, but {:?} is checked out",
            branch.trim()
        ));
    }
    let status = git.output(sh, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(format!(
            "the working tree has changes that are not committed, which a push would leave out:\n{status}"
        ));
    }
    fingerprint(sh, git, Content::Head)
}

/// Refuses unless each sibling is there, without changes and with its HEAD on its upstream, so the release can be built
/// again from what others can fetch; returns each sibling's HEAD commit.
pub(crate) fn siblings_ready(sh: &mut dyn Shell, git: &Git) -> Result<BTreeMap<String, String>, String> {
    let mut commits = BTreeMap::new();
    for name in SIBLINGS {
        let sibling = git.sibling(name);
        if !sibling.dir.exists() {
            return Err(format!("../{name} is missing, which the release builds crates from"));
        }
        let status = sibling.output(sh, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
            return Err(format!(
                "../{name} has changes that are not committed, which no one could build the release from again:\n{status}"
            ));
        }
        sibling.output(sh, &["fetch", "--quiet"])?;
        let upstream = sibling
            .output(
                sh,
                &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
            )
            .map_err(|_| format!("../{name}'s branch has no upstream to fetch the release's sources from"))?;
        if sibling
            .output(sh, &["merge-base", "--is-ancestor", "HEAD", "@{upstream}"])
            .is_err()
        {
            return Err(format!(
                "../{name} has commits that {} lacks; push them first",
                upstream.trim()
            ));
        }
        commits.insert(name.to_string(), sibling.object(sh, &["rev-parse", "HEAD"])?);
    }
    Ok(commits)
}

/// Pushes the release commit and version tag atomically, after all draft assets have been uploaded; `moved` lets the
/// tag replace origin's, for a version published anew from a later commit.
pub(crate) fn push(sh: &mut dyn Shell, git: &Git, tag: &str, moved: bool) -> Result<(), String> {
    ready(sh, git)?;
    let tag = format!("{}refs/tags/{tag}", if moved { "+" } else { "" });
    sh.run(&mut git.command(&["push", "--atomic", "origin", BRANCH, &tag]))?;
    println!("pushed {BRANCH} {tag}");
    Ok(())
}

#[cfg(test)]
#[path = "source_tests.rs"]
pub(crate) mod tests;
