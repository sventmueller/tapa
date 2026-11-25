//! The folder tree of a project panel: each folder read when it is opened, kept up to date from the watcher.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use ignore::IncrementalIgnore;

use crate::order::NameKey;
use crate::rules;
use crate::watch::{Changes, WatchEvent};

/// Finder's per-folder view settings, never project content.
const FINDER_STORE: &str = ".DS_Store";

/// What an entry is; for a symbolic link, what it points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A regular file.
    File,
    /// A folder.
    Dir,
    /// Anything else, such as a socket, or the missing target of a broken link.
    Other,
}

/// How a symbolic link resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Link {
    /// To an existing file or folder.
    Resolved,
    /// To nothing.
    Broken,
    /// To a folder that contains the link, so opening it would never end.
    Cycle,
}

/// A file or folder as the tree lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The file name.
    pub name: String,
    /// The full path, through the links the user opened on the way.
    pub path: PathBuf,
    /// What it is, or for a link what it points to.
    pub kind: Kind,
    /// How it resolves, if it is a symbolic link.
    pub(crate) link: Option<Link>,
    /// The size in bytes of a file; 0 for anything else.
    pub(crate) size: u64,
    /// When the content last changed.
    pub(crate) modified: Option<SystemTime>,
    /// Hidden by `.gitignore`, `.ignore`, `.git/info/exclude` or the global excludes.
    pub(crate) is_ignored: bool,
    /// Hidden by the system with the hidden flag.
    pub(crate) is_hidden: bool,
}

impl Entry {
    /// Whether the tree can list what is inside.
    pub fn can_expand(&self) -> bool {
        self.kind == Kind::Dir && self.link != Some(Link::Cycle)
    }
}

/// What the tree lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeOptions {
    /// Also list ignored and hidden entries, marked as such; `.git` and `.DS_Store` stay out regardless.
    pub show_ignored: bool,
}

/// The folders of one project the user has opened, with their entries: folders first, then in the Finder's order.
pub struct Tree {
    root: PathBuf,
    options: TreeOptions,
    rules: IncrementalIgnore,
    loaded: HashMap<PathBuf, Arc<[Entry]>>,
}

impl Tree {
    /// The tree of the folder `root`, with nothing read yet.
    pub fn new(root: &Path, options: TreeOptions) -> io::Result<Tree> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, root.display().to_string()));
        }
        Ok(Tree {
            rules: rules::matcher(&root),
            root,
            options,
            loaded: HashMap::new(),
        })
    }

    /// The project folder with links resolved; the paths of all entries start with it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The entries of `dir` if it has been read.
    pub(crate) fn children(&self, dir: &Path) -> Option<&[Entry]> {
        self.loaded.get(dir).map(|entries| &**entries)
    }

    /// The entries of `dir` if it has been read, shared with the tree.
    pub fn listed(&self, dir: &Path) -> Option<Arc<[Entry]>> {
        self.loaded.get(dir).cloned()
    }

    /// The entries of `dir`, the root or a folder below it, read now unless the tree already holds them.
    ///
    /// ```
    /// use term_project::{Kind, Tree, TreeOptions};
    ///
    /// let folder = std::env::temp_dir().join(format!("tree-expand-{}", std::process::id()));
    /// std::fs::create_dir_all(folder.join("src")).unwrap();
    /// std::fs::write(folder.join("README.md"), "").unwrap();
    /// let mut tree = Tree::new(&folder, TreeOptions::default()).unwrap();
    /// let root = tree.root().to_path_buf();
    /// let listed: Vec<_> = tree.expand(&root).unwrap().iter().map(|e| (e.name.clone(), e.kind)).collect();
    /// assert_eq!(listed, [("src".to_string(), Kind::Dir), ("README.md".to_string(), Kind::File)]);
    /// std::fs::remove_dir_all(folder).unwrap();
    /// ```
    pub fn expand(&mut self, dir: &Path) -> io::Result<&[Entry]> {
        if !self.loaded.contains_key(dir) {
            let entries = self.read(dir)?;
            self.loaded.insert(dir.to_path_buf(), entries.into());
        }
        Ok(self.children(dir).unwrap_or_default())
    }

    /// Forgets `dir` and every folder below it, which then no longer follow changes on disk.
    pub fn collapse(&mut self, dir: &Path) {
        self.loaded.retain(|path, _| !path.starts_with(dir));
    }

    /// Lists with `options` from now on; the folders whose entries changed.
    pub fn set_options(&mut self, options: TreeOptions) -> Vec<PathBuf> {
        if options == self.options {
            return Vec::new();
        }
        self.options = options;
        self.reload_all()
    }

    /// Brings the folders read so far up to date with what the watcher saw; the folders whose entries changed, with
    /// those it no longer follows because their parent no longer lists them.
    pub fn apply(&mut self, event: &WatchEvent) -> Vec<PathBuf> {
        match event {
            WatchEvent::Rescan(_) => self.reload_all(),
            WatchEvent::Changed(changes) => self.patch(changes),
        }
    }

    fn patch(&mut self, changes: &Changes) -> Vec<PathBuf> {
        if changes.rules_changed {
            return self.reload_all();
        }
        let mut stale: Vec<PathBuf> = changes
            .dirs
            .iter()
            .filter(|dir| self.loaded.contains_key(*dir))
            .cloned()
            .collect();
        for subtree in &changes.subtrees {
            stale.extend(self.loaded.keys().filter(|dir| dir.starts_with(subtree)).cloned());
        }
        self.reload(stale)
    }

    fn reload_all(&mut self) -> Vec<PathBuf> {
        self.rules = rules::matcher(&self.root);
        let dirs = self.loaded.keys().cloned().collect();
        self.reload(dirs)
    }

    fn reload(&mut self, mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
        dirs.sort_by_key(|dir| dir.components().count());
        dirs.dedup();
        let mut changed = Vec::new();
        for dir in dirs {
            if !self.loaded.contains_key(&dir) {
                continue;
            }
            match self.read(&dir) {
                Ok(entries) if self.loaded.get(&dir).is_some_and(|old| **old == *entries) => {}
                Ok(entries) => {
                    self.loaded.insert(dir.clone(), entries.into());
                    changed.push(dir);
                }
                Err(_) => {
                    self.loaded.remove(&dir);
                    changed.push(dir);
                }
            }
        }
        changed.extend(self.prune());
        changed
    }

    /// Drops folders whose parent was dropped or no longer lists them as a folder to open; the folders dropped.
    fn prune(&mut self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self.loaded.keys().filter(|dir| **dir != self.root).cloned().collect();
        dirs.sort_by_key(|dir| dir.components().count());
        let mut dropped = Vec::new();
        for dir in dirs {
            let listed = dir
                .parent()
                .and_then(|parent| self.loaded.get(parent))
                .is_some_and(|entries| entries.iter().any(|e| e.path == dir && e.can_expand()));
            if !listed {
                self.loaded.remove(&dir);
                dropped.push(dir);
            }
        }
        dropped
    }

    fn read(&mut self, dir: &Path) -> io::Result<Vec<Entry>> {
        let relative = dir
            .strip_prefix(&self.root)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "outside the project"))?
            .to_path_buf();
        let mut real_dir = None;
        let mut entries = Vec::new();
        for item in fs::read_dir(dir)? {
            let Ok(item) = item else { continue };
            let name = item.file_name();
            // Without following a link, as git and the walks of the index and the search see it.
            let Ok(own) = item.metadata() else { continue };
            if name == rules::GIT_DIR || name == FINDER_STORE {
                continue;
            }
            let is_ignored = self.rules.matched(relative.join(&name), own.is_dir()).is_ignore();
            let is_hidden = flagged_hidden(&own);
            if !self.options.show_ignored && (is_ignored || is_hidden) {
                continue;
            }
            let path = item.path();
            let (kind, link, metadata) = if own.file_type().is_symlink() {
                match fs::metadata(&path) {
                    Ok(target) => {
                        let link = if target.is_dir() && loops(&path, dir, &mut real_dir) {
                            Link::Cycle
                        } else {
                            Link::Resolved
                        };
                        (kind_of(&target), Some(link), target)
                    }
                    Err(_) => (Kind::Other, Some(Link::Broken), own),
                }
            } else {
                (kind_of(&own), None, own)
            };
            entries.push(Entry {
                name: name.to_string_lossy().into_owned(),
                path,
                kind,
                link,
                size: if kind == Kind::File { metadata.len() } else { 0 },
                modified: metadata.modified().ok(),
                is_ignored,
                is_hidden,
            });
        }
        entries.sort_by_cached_key(|entry| (entry.kind != Kind::Dir, NameKey::new(&entry.name)));
        Ok(entries)
    }
}

fn kind_of(metadata: &fs::Metadata) -> Kind {
    if metadata.is_dir() {
        Kind::Dir
    } else if metadata.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

/// Whether the folder `link` points to contains `dir`, the folder the link is in.
fn loops(link: &Path, dir: &Path, real_dir: &mut Option<PathBuf>) -> bool {
    let Ok(target) = link.canonicalize() else { return false };
    if real_dir.is_none() {
        *real_dir = dir.canonicalize().ok();
    }
    real_dir.as_ref().is_some_and(|real| real.starts_with(&target))
}

#[cfg(target_os = "macos")]
fn flagged_hidden(metadata: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    /// `UF_HIDDEN` from `<sys/stat.h>`, set by `chflags hidden`.
    const UF_HIDDEN: u32 = 0x8000;
    metadata.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(target_os = "macos"))]
fn flagged_hidden(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::os::unix::fs::symlink;

    use super::*;
    use crate::testing::{scratch, write};

    fn project(name: &str) -> term_base::testing::Scratch {
        let root = scratch(name);
        write(&root, ".git/info/exclude", "excluded.txt\n");
        write(&root, ".gitignore", "target/\n*.log\n");
        write(&root, ".ignore", "secret/\n");
        write(&root, "src/main.rs", "fn main() {}\n");
        write(&root, "src/.gitignore", "gen.rs\n");
        write(&root, "src/gen.rs", "");
        write(&root, "target/out.bin", "");
        write(&root, "secret/key", "");
        for name in [
            "debug.log",
            "excluded.txt",
            ".env",
            ".DS_Store",
            "B.txt",
            "a10.txt",
            "a2.txt",
        ] {
            write(&root, name, "x");
        }
        fs::create_dir(root.join("docs")).unwrap();
        root
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn a_folder_lists_folders_first_in_finder_order_without_what_is_ignored() {
        let root = project("tree-order");
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        let entries = tree.expand(&root).unwrap();
        assert_eq!(
            names(entries),
            [
                "docs",
                "src",
                ".env",
                ".gitignore",
                ".ignore",
                "a2.txt",
                "a10.txt",
                "B.txt"
            ]
        );
        assert!(entries.iter().all(|e| !e.is_ignored && !e.is_hidden));
        let b = entries.iter().find(|e| e.name == "B.txt").unwrap();
        assert_eq!((b.kind, b.size, b.link), (Kind::File, 1, None));
        assert!(b.modified.is_some());
        assert_eq!(
            names(tree.expand(&root.join("src")).unwrap()),
            [".gitignore", "main.rs"]
        );
    }

    #[test]
    fn showing_ignored_entries_marks_them_and_still_leaves_out_git_and_ds_store() {
        let root = project("tree-ignored");
        write(&root, "flagged.txt", "x");
        let status = std::process::Command::new("chflags")
            .arg("hidden")
            .arg(root.join("flagged.txt"))
            .status()
            .unwrap();
        assert!(status.success());
        let mut tree = Tree::new(&root, TreeOptions { show_ignored: true }).unwrap();
        let entries = tree.expand(&root).unwrap().to_vec();
        let ignored: BTreeSet<&str> = entries
            .iter()
            .filter(|e| e.is_ignored)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(
            ignored,
            BTreeSet::from(["debug.log", "excluded.txt", "secret", "target"])
        );
        let hidden: Vec<&str> = entries
            .iter()
            .filter(|e| e.is_hidden)
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(hidden, ["flagged.txt"]);
        assert!(!entries.iter().any(|e| e.name == ".git" || e.name == ".DS_Store"));
        let inside = tree.expand(&root.join("target")).unwrap();
        assert!(inside.iter().all(|e| e.is_ignored), "{inside:?}");
    }

    #[test]
    fn links_are_marked_and_a_link_to_an_enclosing_folder_does_not_open() {
        let root = project("tree-links");
        symlink(&root, root.join("src/up")).unwrap();
        symlink(root.join("src/main.rs"), root.join("main-link.rs")).unwrap();
        symlink(root.join("missing"), root.join("dangling")).unwrap();
        symlink(root.join("src"), root.join("docs/src-link")).unwrap();
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        let top = tree.expand(&root).unwrap().to_vec();
        let file = top.iter().find(|e| e.name == "main-link.rs").unwrap();
        assert_eq!(
            (file.kind, file.link, file.size),
            (Kind::File, Some(Link::Resolved), 13)
        );
        let broken = top.iter().find(|e| e.name == "dangling").unwrap();
        assert_eq!((broken.kind, broken.link), (Kind::Other, Some(Link::Broken)));
        let up = tree
            .expand(&root.join("src"))
            .unwrap()
            .iter()
            .find(|e| e.name == "up")
            .unwrap();
        assert_eq!(
            (up.kind, up.link, up.can_expand()),
            (Kind::Dir, Some(Link::Cycle), false)
        );
        let across = tree.expand(&root.join("docs")).unwrap()[0].clone();
        assert_eq!((across.link, across.can_expand()), (Some(Link::Resolved), true));
        let through = tree.expand(&across.path).unwrap();
        assert!(names(through).contains(&"main.rs"));
    }

    #[test]
    fn changes_from_the_watcher_patch_only_folders_already_read() {
        let root = project("tree-patch");
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        tree.expand(&root).unwrap();
        tree.expand(&root.join("docs")).unwrap();
        write(&root, "src/lib.rs", "");
        write(&root, "docs/guide.md", "");
        let changes = Changes {
            dirs: BTreeSet::from([root.join("src"), root.join("docs")]),
            ..Changes::default()
        };
        assert_eq!(tree.apply(&WatchEvent::Changed(changes)), [root.join("docs")]);
        assert_eq!(names(tree.children(&root.join("docs")).unwrap()), ["guide.md"]);
        assert!(tree.children(&root.join("src")).is_none());

        fs::remove_dir_all(root.join("docs")).unwrap();
        let changes = Changes {
            dirs: BTreeSet::from([root.to_path_buf(), root.join("docs")]),
            ..Changes::default()
        };
        assert!(tree.apply(&WatchEvent::Changed(changes)).contains(&root.to_path_buf()));
        assert!(tree.children(&root.join("docs")).is_none());
        assert!(!names(tree.children(&root).unwrap()).contains(&"docs"));
    }

    #[test]
    fn a_changed_ignore_file_or_a_rescan_reads_every_open_folder_again() {
        let root = project("tree-rules");
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        tree.expand(&root).unwrap();
        write(&root, ".gitignore", "target/\n*.log\nB.txt\n");
        let changes = Changes {
            rules_changed: true,
            ..Changes::default()
        };
        assert_eq!(tree.apply(&WatchEvent::Changed(changes)), std::slice::from_ref(&root));
        assert!(!names(tree.children(&root).unwrap()).contains(&"B.txt"));

        write(&root, "new.txt", "");
        tree.apply(&WatchEvent::Rescan(crate::RescanReason::Dropped));
        assert!(names(tree.children(&root).unwrap()).contains(&"new.txt"));
    }

    #[test]
    fn a_folder_replaced_as_a_whole_is_read_again_with_its_open_subfolders() {
        let root = project("tree-subtree");
        write(&root, "docs/a/old.txt", "");
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        tree.expand(&root).unwrap();
        tree.expand(&root.join("docs")).unwrap();
        tree.expand(&root.join("docs/a")).unwrap();
        fs::remove_file(root.join("docs/a/old.txt")).unwrap();
        write(&root, "docs/a/new.txt", "");
        let changes = Changes {
            subtrees: BTreeSet::from([root.join("docs")]),
            ..Changes::default()
        };
        assert!(tree.apply(&WatchEvent::Changed(changes)).contains(&root.join("docs/a")));
        assert_eq!(names(tree.children(&root.join("docs/a")).unwrap()), ["new.txt"]);
    }

    #[test]
    fn showing_ignored_entries_again_reports_the_folders_that_changed() {
        let root = project("tree-toggle");
        let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
        tree.expand(&root).unwrap();
        tree.expand(&root.join("docs")).unwrap();
        let changed = tree.set_options(TreeOptions { show_ignored: true });
        assert_eq!(changed, std::slice::from_ref(&root));
        assert!(names(tree.children(&root).unwrap()).contains(&"target"));
        assert!(tree.set_options(TreeOptions { show_ignored: true }).is_empty());
    }

    #[test]
    #[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
    fn a_folder_of_many_entries_lists_within_its_share_of_the_budget() {
        let root = scratch("tree-wide");
        for n in 0..5_000 {
            fs::write(root.join(format!("file{n}.txt")), "").unwrap();
        }
        let mut times = Vec::new();
        let mut listed = None;
        for _ in 0..5 {
            let mut tree = Tree::new(&root, TreeOptions::default()).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(tree.expand(&root).unwrap().len(), 5_000);
            times.push(started.elapsed());
            listed = Some(tree);
        }
        let tree = listed.unwrap();
        term_base::perf::durations("term-project: listing a folder of 5 000 entries", &times);
        times.sort();
        let took = times[times.len() / 2];
        let bound = std::time::Duration::from_millis(100) * crate::testing::walk_factor();
        assert!(took < bound, "{took:?}");
        eprintln!("listed 5 000 entries in {took:?}");
        assert_eq!(tree.children(&root).unwrap()[1].name, "file1.txt");
    }
}
