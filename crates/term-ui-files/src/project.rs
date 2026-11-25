//! The folder of a workspace as its file list reads it: the tree, read one folder at a time on the disk lane, and the
//! watcher that keeps it current, which runs only while the list exists. What either read reaches the list by its id
//! on the main thread; changes on disk go to the editors and, through the host, to the Git panel.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use term_base::LockIgnoringPoison;
use term_base::main_soon;
use term_project::{Entry, Tree, TreeOptions, WatchEvent, Watcher};

/// A folder of the tree with its entries as last read, `None` once the tree no longer follows it.
pub(crate) type Listing = (PathBuf, Option<Arc<[Entry]>>);

pub(crate) struct Project {
    root: PathBuf,
    /// The list the readings go to.
    panel: u64,
    /// Locked only off the main thread, by the watcher and the disk lane, which read the disk while they hold it.
    tree: Arc<Mutex<Tree>>,
    _watcher: Option<Watcher>,
}

impl Project {
    /// Starts reading the top of `root` for the list `panel` and watching it; the watcher refuses `/` and the home
    /// folder itself, whose list then follows only what the user opens.
    pub(crate) fn open(root: &Path, panel: u64, options: TreeOptions) -> std::io::Result<Project> {
        let tree = Tree::new(root, options)?;
        let root = tree.root().to_path_buf();
        let tree = Arc::new(Mutex::new(tree));
        let watcher = {
            let (tree, root) = (tree.clone(), root.clone());
            Watcher::start(&root.clone(), move |event| changed(&tree, &root, panel, &event))
        };
        let watcher = watcher
            .inspect_err(|error| term_base::diag::report("file watcher", error))
            .ok();
        let project = Project {
            root,
            panel,
            tree,
            _watcher: watcher,
        };
        project.expand(&project.root);
        Ok(project)
    }

    /// Whether the watcher runs, which it refuses for `/` and the home folder.
    #[cfg(feature = "selftest")]
    pub(crate) fn is_watching(&self) -> bool {
        self._watcher.is_some()
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Reads the folder `dir` unless the tree holds it, and hands its entries to the list.
    pub(crate) fn expand(&self, dir: &Path) {
        let dir = dir.to_path_buf();
        self.on_disk(move |tree| {
            // A folder that cannot be read lists as none.
            let _ = tree.expand(&dir);
            vec![dir]
        });
    }

    /// Forgets `dir` and every folder below it, after what was asked before.
    pub(crate) fn collapse(&self, dir: &Path) {
        let dir = dir.to_path_buf();
        self.on_disk(move |tree| {
            tree.collapse(&dir);
            Vec::new()
        });
    }

    /// Lists with `options` from now on, and hands the folders that changed to the list.
    pub(crate) fn set_options(&self, options: TreeOptions) {
        self.on_disk(move |tree| tree.set_options(options));
    }

    /// Runs `work` with the tree on the disk lane and hands the folders it names, as they are then, to the list.
    fn on_disk(&self, work: impl FnOnce(&mut Tree) -> Vec<PathBuf> + Send + 'static) {
        let (tree, panel) = (self.tree.clone(), self.panel);
        let queued = term_base::lane::DISK.try_run(move || {
            let mut tree = tree.lock_ignoring_poison();
            let dirs = work(&mut tree);
            if !dirs.is_empty() {
                let listings = listings(&tree, dirs);
                main_soon(move || crate::panel::listed(panel, &listings));
            }
        });
        if queued.is_err() {
            term_base::diag::report("reading a folder", "the disk lane is full");
        }
    }
}

/// What the watcher saw below `root`: the tree follows it and the list shows what changed, the editors look at their
/// files and the change marks at the commit, and the host hears of every change.
fn changed(tree: &Mutex<Tree>, root: &Path, panel: u64, event: &WatchEvent) {
    {
        let mut tree = tree.lock_ignoring_poison();
        let dirs = tree.apply(event);
        if !dirs.is_empty() {
            let listings = listings(&tree, dirs);
            // Queued while the lock is held, so the main thread gets the readings in the order they were made.
            main_soon(move || {
                let dirs: Vec<PathBuf> = listings.iter().map(|(dir, _)| dir.clone()).collect();
                term_ui_editor::files_changed(&dirs);
                crate::panel::listed(panel, &listings);
            });
        }
    }
    let git = match event {
        WatchEvent::Changed(changes) => changes.git_changed,
        WatchEvent::Rescan(_) => true,
    };
    let root = root.to_path_buf();
    main_soon(move || {
        if git {
            term_ui_editor::git_changed(&root);
        }
        if let Some(host) = crate::host::installed() {
            host.changed(&root);
        }
    });
}

fn listings(tree: &Tree, dirs: Vec<PathBuf>) -> Vec<Listing> {
    dirs.into_iter()
        .map(|dir| {
            let entries = tree.listed(&dir);
            (dir, entries)
        })
        .collect()
}
