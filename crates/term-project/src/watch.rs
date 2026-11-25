//! Changes on disk below a project folder, from one FSEvents stream for the whole tree, gathered for about 50 ms and
//! reported per folder.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use notify::event::{EventKind, ModifyKind};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::rules;

/// How long changes gather before they are reported together.
const WINDOW: Duration = Duration::from_millis(50);

/// Raw events that may wait for the gathering thread; more mean events were lost.
const QUEUE: usize = 4096;

/// Folders one report may name; more and a rescan is cheaper than patching.
const MAX_DIRS: usize = 10_000;

/// Files one report names; beyond them only their folders are reported.
const MAX_FILES: usize = 10_000;

/// How a file changed, as far as it can be told once the report is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// It appeared, also by a rename.
    Created,
    /// Its content or metadata changed.
    Changed,
    /// It is gone, also by a rename.
    Deleted,
}

/// What changed in one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Folders whose entries changed: something in them appeared, went, was renamed or written.
    pub dirs: BTreeSet<PathBuf>,
    /// Folders that appeared as a whole, created or moved in, whose content may have come without events of its own.
    pub subtrees: BTreeSet<PathBuf>,
    /// An ignore file changed, so any folder may now show or hide other entries.
    pub(crate) rules_changed: bool,
    /// Something in a `.git` folder changed, such as the branch, the index or a ref.
    pub git_changed: bool,
    /// The files that changed, outside `.git`, up to `MAX_FILES` of them; folders are not listed here.
    pub files: BTreeMap<PathBuf, FileChange>,
}

/// Why the watcher can no longer say what changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RescanReason {
    /// The system dropped events or asked for the folders to be scanned again.
    Dropped,
    /// More changed at once than the watcher keeps track of.
    Overflow,
    /// The stream of events failed.
    Failed(String),
}

/// A report from the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// These folders changed.
    Changed(Changes),
    /// Events were missed: whatever was read from disk must be read again.
    Rescan(RescanReason),
}

/// Watches a project folder and everything below it until dropped.
pub struct Watcher {
    #[cfg(test)]
    root: PathBuf,
    open: Arc<AtomicBool>,
    _stream: RecommendedWatcher,
}

impl Watcher {
    /// Starts watching `root`; `on_event` runs on a thread of the watcher's own, never after the watcher is dropped.
    /// The file system's root and the home folder itself are refused, as a stream over either reports every change
    /// on the Mac or in all of the user's files.
    pub fn start(root: &Path, on_event: impl FnMut(WatchEvent) + Send + 'static) -> io::Result<Watcher> {
        let root = root.canonicalize()?;
        if is_too_wide(&root) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the root or home folder is not watched",
            ));
        }
        let (events, received) = sync_channel(QUEUE);
        let lost = Arc::new(AtomicBool::new(false));
        let open = Arc::new(AtomicBool::new(true));
        let handler = {
            let lost = lost.clone();
            move |event: notify::Result<notify::Event>| {
                if let Err(TrySendError::Full(_)) = events.try_send(event) {
                    lost.store(true, Ordering::Relaxed);
                }
            }
        };
        let mut stream = notify::recommended_watcher(handler).map_err(io::Error::other)?;
        stream
            .watch(&root, RecursiveMode::Recursive)
            .map_err(io::Error::other)?;
        let gather = Gather {
            root: root.clone(),
            lost,
            open: open.clone(),
        };
        std::thread::Builder::new().name("term-watch".into()).spawn(move || {
            term_base::priority::set_qos(term_base::priority::Qos::Utility);
            gather.run(&received, on_event);
        })?;
        Ok(Watcher {
            #[cfg(test)]
            root,
            open,
            _stream: stream,
        })
    }

    /// The watched folder with links resolved, as the paths in reports start.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

/// Whether `root`, with links resolved, is `/` or the home folder.
fn is_too_wide(root: &Path) -> bool {
    let home = term_base::home();
    let home = home.canonicalize().unwrap_or(home);
    root.parent().is_none() || root == home
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.open.store(false, Ordering::Relaxed);
    }
}

struct Gather {
    root: PathBuf,
    lost: Arc<AtomicBool>,
    open: Arc<AtomicBool>,
}

impl Gather {
    fn run(self, received: &Receiver<notify::Result<notify::Event>>, mut on_event: impl FnMut(WatchEvent)) {
        while let Ok(first) = received.recv() {
            let mut batch = Batch::default();
            batch.add(&self.root, first);
            let deadline = Instant::now() + WINDOW;
            let mut ended = false;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match received.recv_timeout(left) {
                    Ok(event) => batch.add(&self.root, event),
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        ended = true;
                        break;
                    }
                }
            }
            if self.lost.swap(false, Ordering::Relaxed) {
                batch.rescan.get_or_insert(RescanReason::Overflow);
            }
            if !self.open.load(Ordering::Relaxed) {
                return;
            }
            if let Some(event) = batch.finish() {
                term_base::lane::contained(|| on_event(event));
            }
            if ended {
                return;
            }
        }
    }
}

#[derive(Debug, Default)]
struct Batch {
    changes: Changes,
    rescan: Option<RescanReason>,
}

impl Batch {
    fn add(&mut self, root: &Path, event: notify::Result<notify::Event>) {
        if self.rescan.is_some() {
            return;
        }
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                self.rescan = Some(RescanReason::Failed(error.to_string()));
                return;
            }
        };
        if event.need_rescan() {
            self.rescan = Some(RescanReason::Dropped);
            return;
        }
        let whole = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        let change = match event.kind {
            EventKind::Access(_) => None,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => Some(FileChange::Created),
            EventKind::Remove(_) => Some(FileChange::Deleted),
            _ => Some(FileChange::Changed),
        };
        for path in &event.paths {
            self.path(root, path, whole);
            if let Some(change) = change {
                self.file(root, path, change);
            }
        }
        if self.changes.dirs.len() + self.changes.subtrees.len() > MAX_DIRS {
            self.rescan = Some(RescanReason::Overflow);
        }
    }

    fn path(&mut self, root: &Path, path: &Path, whole: bool) {
        let Ok(relative) = path.strip_prefix(root) else { return };
        if rules::in_git_dir(relative) {
            self.changes.git_changed = true;
            self.changes.rules_changed |= relative.ends_with(".git/info/exclude");
            return;
        }
        if path
            .file_name()
            .is_some_and(|name| name == ".gitignore" || name == ".ignore")
        {
            self.changes.rules_changed = true;
        }
        if relative.as_os_str().is_empty() {
            self.changes.dirs.insert(root.to_path_buf());
            return;
        }
        if let Some(parent) = path.parent() {
            self.changes.dirs.insert(parent.to_path_buf());
        }
        if whole && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
            self.changes.subtrees.insert(path.to_path_buf());
        }
    }

    /// Notes a change of `path`; a file created in this report stays created whatever follows.
    fn file(&mut self, root: &Path, path: &Path, change: FileChange) {
        let Ok(relative) = path.strip_prefix(root) else { return };
        if relative.as_os_str().is_empty() || rules::in_git_dir(relative) {
            return;
        }
        if self.changes.files.len() >= MAX_FILES && !self.changes.files.contains_key(path) {
            return;
        }
        let entry = self.changes.files.entry(path.to_path_buf()).or_insert(change);
        if !(*entry == FileChange::Created && change == FileChange::Changed) {
            *entry = change;
        }
    }

    fn finish(mut self) -> Option<WatchEvent> {
        if let Some(reason) = self.rescan {
            return Some(WatchEvent::Rescan(reason));
        }
        // The system merges flags of quick successions, so whether a file is there decides what happened to it.
        self.changes
            .files
            .retain(|path, change| match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => false,
                Ok(_) => {
                    if *change == FileChange::Deleted {
                        *change = FileChange::Changed;
                    }
                    true
                }
                Err(_) => {
                    *change = FileChange::Deleted;
                    true
                }
            });
        (self.changes != Changes::default()).then_some(WatchEvent::Changed(self.changes))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, channel};

    use notify::event::{CreateKind, DataChange, Flag, RemoveKind};

    use super::*;
    use crate::testing::{scratch, write};

    fn event(kind: EventKind, path: PathBuf) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(kind).add_path(path))
    }

    fn written() -> EventKind {
        EventKind::Modify(ModifyKind::Data(DataChange::Any))
    }

    #[test]
    fn events_are_reported_per_folder_and_git_internals_apart() {
        let root = scratch("watch-batch");
        write(&root, "src/moved/inner.rs", "");
        let mut batch = Batch::default();
        batch.add(&root, event(written(), root.join("src/a.rs")));
        batch.add(&root, event(written(), root.join("src/b.rs")));
        batch.add(&root, event(EventKind::Remove(RemoveKind::File), root.join("top.txt")));
        batch.add(
            &root,
            event(EventKind::Create(CreateKind::Folder), root.join("src/moved")),
        );
        batch.add(&root, event(written(), root.join(".git/index")));
        batch.add(&root, event(written(), PathBuf::from("/elsewhere/file")));
        let Some(WatchEvent::Changed(changes)) = batch.finish() else {
            panic!()
        };
        assert_eq!(changes.dirs, BTreeSet::from([root.to_path_buf(), root.join("src")]));
        assert_eq!(changes.subtrees, BTreeSet::from([root.join("src/moved")]));
        assert!(changes.git_changed);
        assert!(!changes.rules_changed);
        assert!(
            !changes
                .files
                .keys()
                .any(|p| p.starts_with(root.join(".git")) || p == &root.join("src/moved"))
        );
    }

    #[test]
    fn files_are_reported_as_created_changed_or_deleted_by_what_is_on_disk() {
        let root = scratch("watch-files");
        write(&root, "kept.rs", "");
        write(&root, "new.rs", "");
        let mut batch = Batch::default();
        batch.add(&root, event(EventKind::Create(CreateKind::File), root.join("new.rs")));
        batch.add(&root, event(written(), root.join("new.rs")));
        batch.add(&root, event(written(), root.join("kept.rs")));
        batch.add(&root, event(written(), root.join("gone.rs")));
        batch.add(&root, event(EventKind::Remove(RemoveKind::File), root.join("kept.rs")));
        let Some(WatchEvent::Changed(changes)) = batch.finish() else {
            panic!()
        };
        assert_eq!(
            changes.files,
            BTreeMap::from([
                (root.join("gone.rs"), FileChange::Deleted),
                (root.join("kept.rs"), FileChange::Changed),
                (root.join("new.rs"), FileChange::Created),
            ])
        );
    }

    #[test]
    fn a_changed_ignore_file_is_flagged() {
        let root = scratch("watch-rules");
        for changed in [".gitignore", "src/.ignore", ".git/info/exclude"] {
            let mut batch = Batch::default();
            batch.add(&root, event(written(), root.join(changed)));
            let Some(WatchEvent::Changed(changes)) = batch.finish() else {
                panic!()
            };
            assert!(changes.rules_changed, "{changed}");
        }
    }

    #[test]
    fn dropped_events_failures_and_floods_ask_for_a_rescan() {
        let root = scratch("watch-rescan");
        let mut batch = Batch::default();
        batch.add(&root, event(written(), root.join("a.txt")));
        batch.add(&root, Ok(notify::Event::new(EventKind::Other).set_flag(Flag::Rescan)));
        assert_eq!(batch.finish(), Some(WatchEvent::Rescan(RescanReason::Dropped)));

        let mut batch = Batch::default();
        batch.add(&root, Err(notify::Error::generic("stream ended")));
        assert!(matches!(
            batch.finish(),
            Some(WatchEvent::Rescan(RescanReason::Failed(_)))
        ));

        let mut batch = Batch::default();
        for n in 0..=MAX_DIRS {
            batch.add(&root, event(written(), root.join(format!("d{n}/f"))));
        }
        assert_eq!(batch.finish(), Some(WatchEvent::Rescan(RescanReason::Overflow)));
        assert_eq!(Batch::default().finish(), None);
    }

    #[test]
    fn the_root_and_the_home_folder_are_never_watched() {
        for wide in [PathBuf::from("/"), term_base::home(), term_base::home().join(".")] {
            let started = Watcher::start(&wide, |_| {});
            assert_eq!(
                started.err().map(|error| error.kind()),
                Some(io::ErrorKind::InvalidInput),
                "{}",
                wide.display()
            );
        }
        assert!(!is_too_wide(&term_base::home().join("project")));
    }

    /// Merges reports until `done` holds for what arrived so far, or fails after five seconds.
    fn wait_for(received: &Receiver<WatchEvent>, done: impl Fn(&Changes) -> bool) -> Changes {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Changes::default();
        while !done(&seen) {
            let left = deadline.saturating_duration_since(Instant::now());
            match received.recv_timeout(left) {
                Ok(WatchEvent::Changed(changes)) => {
                    seen.dirs.extend(changes.dirs);
                    seen.subtrees.extend(changes.subtrees);
                    seen.rules_changed |= changes.rules_changed;
                    seen.git_changed |= changes.git_changed;
                }
                Ok(WatchEvent::Rescan(reason)) => panic!("rescan: {reason:?}"),
                Err(_) => panic!("no report in time; so far {seen:?}"),
            }
        }
        seen
    }

    #[test]
    fn the_stream_reports_files_folders_moved_in_and_git_changes() {
        let root = scratch("watch-live");
        let outside = scratch("watch-live-outside");
        write(&root, "src/keep.rs", "");
        write(&outside, "pack/inner/file.txt", "");
        std::fs::create_dir(root.join(".git")).unwrap();
        let (sender, received) = channel();
        let watcher = Watcher::start(&root, move |event| {
            let _ = sender.send(event);
        })
        .unwrap();
        assert_eq!(watcher.root(), root.path());

        write(&root, "src/new.rs", "");
        wait_for(&received, |seen| seen.dirs.contains(&root.join("src")));

        std::fs::rename(outside.join("pack"), root.join("pack")).unwrap();
        wait_for(&received, |seen| seen.subtrees.contains(&root.join("pack")));

        write(&root, ".git/HEAD", "ref: refs/heads/main\n");
        write(&root, ".gitignore", "*.log\n");
        wait_for(&received, |seen| seen.git_changed && seen.rules_changed);
        drop(watcher);
    }
}
