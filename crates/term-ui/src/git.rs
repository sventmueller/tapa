//! The branch of a folder's repository, read from the files in `.git` (E16): no `git` runs, so nothing a project
//! configures, such as an fsmonitor hook, runs for the sidebar or the input field's chips.

use std::path::{Path, PathBuf};

/// The `.git` folder of the repository `folder` is in, following a worktree's or submodule's `gitdir:` file.
fn git_dir(folder: &Path) -> Option<PathBuf> {
    let mut at = Some(folder);
    while let Some(dir) = at {
        let candidate = dir.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate).ok()?;
            let pointed = text.trim().strip_prefix("gitdir:")?.trim();
            let pointed = Path::new(pointed);
            return Some(if pointed.is_absolute() {
                pointed.to_path_buf()
            } else {
                dir.join(pointed)
            });
        }
        at = dir.parent();
    }
    None
}

/// The branch checked out in the repository `folder` is in; the first seven digits of the commit when none is.
pub fn branch(folder: &Path) -> Option<String> {
    head_branch(&git_dir(folder)?)
}

/// A working tree of a repository: its folder and the branch checked out there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// The folder the files are checked out in.
    pub folder: PathBuf,
    /// The branch, or the short commit where none is checked out.
    pub branch: Option<String>,
}

/// The working trees of the repository `folder` is in, the main one first, from `.git/worktrees` as `git worktree`
/// keeps them.
pub fn worktrees(folder: &Path) -> Vec<Worktree> {
    let Some(dir) = git_dir(folder) else {
        return Vec::new();
    };
    let common = std::fs::read_to_string(dir.join("commondir"))
        .ok()
        .map_or_else(|| dir.clone(), |text| relative_to(&dir, text.trim()));
    let common = common.canonicalize().unwrap_or(common);
    let mut found = Vec::new();
    if common.file_name().is_some_and(|name| name == ".git")
        && let Some(main) = common.parent()
    {
        found.push(Worktree {
            folder: main.to_path_buf(),
            branch: head_branch(&common),
        });
    }
    let mut linked: Vec<PathBuf> = std::fs::read_dir(common.join("worktrees"))
        .map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect())
        .unwrap_or_default();
    linked.sort();
    for entry in linked {
        let Ok(gitdir) = std::fs::read_to_string(entry.join("gitdir")) else {
            continue;
        };
        let file = relative_to(&entry, gitdir.trim());
        if let Some(folder) = file.parent() {
            found.push(Worktree {
                folder: folder.to_path_buf(),
                branch: head_branch(&entry),
            });
        }
    }
    found
}

fn relative_to(dir: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        dir.join(path)
    }
}

fn head_branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: ") {
        Some(reference) => Some(reference.strip_prefix("refs/heads/").unwrap_or(reference).to_string()),
        None if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) => Some(head[..7].to_string()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_is_read_from_head_also_in_a_subfolder_and_a_worktree() {
        let repo = term_base::testing::scratch("ui-git");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(branch(&repo.join("src/deep")).as_deref(), Some("feature/x"));
        std::fs::write(repo.join(".git/HEAD"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        assert_eq!(branch(&repo).as_deref(), Some("0123456"));
        let worktree = repo.join("wt");
        std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(repo.join(".git/worktrees/wt/HEAD"), "ref: refs/heads/other\n").unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/wt\n").unwrap();
        assert_eq!(branch(&worktree).as_deref(), Some("other"));
    }

    #[test]
    fn the_worktrees_are_read_from_the_common_git_folder() {
        let repo = term_base::testing::scratch("ui-worktrees");
        let repo = repo.canonicalize().unwrap();
        std::fs::create_dir_all(repo.join(".git/worktrees/wt")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let worktree = repo.join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(repo.join(".git/worktrees/wt/HEAD"), "ref: refs/heads/other\n").unwrap();
        std::fs::write(repo.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        std::fs::write(
            repo.join(".git/worktrees/wt/gitdir"),
            format!("{}\n", worktree.join(".git").display()),
        )
        .unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/wt\n").unwrap();
        let expected = vec![
            Worktree {
                folder: repo.clone(),
                branch: Some("main".into()),
            },
            Worktree {
                folder: worktree.clone(),
                branch: Some("other".into()),
            },
        ];
        assert_eq!(worktrees(&repo), expected);
        assert_eq!(worktrees(&worktree), expected, "the same list from inside the worktree");
    }

    #[test]
    fn a_folder_outside_a_repository_has_no_branch() {
        let dir = term_base::testing::scratch("ui-nogit");
        assert_eq!(branch(&dir), None);
    }
}
