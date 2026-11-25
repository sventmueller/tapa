//! The thread index, the bookmarks and the folders' past sessions as the main thread sees them. Changes go to the disk
//! lane in the order they were made, a thread's newer record taking the place of one still waiting; what is read
//! there is kept here for the main thread.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use term_base::LockIgnoringPoison;
use term_thread::{IndexEntry, ThreadId, ThreadIndex};

use crate::common::util::main_soon;
use crate::{Agent, Resume};

/// How many past sessions of a folder are kept.
const KEPT: usize = 500;

/// A change to the index or the bookmarks.
enum Change {
    /// A thread as it is now, on a subscription or not, under the title the user gave it.
    Record {
        entry: IndexEntry,
        subscription: bool,
        title: Option<String>,
    },
    Rename(ThreadId, String),
    Bookmark(ThreadId, bool),
    /// Reads the bookmarks, which no change came for yet.
    ReadBookmarks,
}

impl Change {
    fn thread(&self) -> Option<&ThreadId> {
        match self {
            Change::Record { entry, .. } => Some(&entry.id),
            Change::Rename(id, _) | Change::Bookmark(id, _) => Some(id),
            Change::ReadBookmarks => None,
        }
    }
}

/// What the main thread keeps of the index and the bookmarks.
#[derive(Default)]
pub(super) struct Store {
    /// Changes made while the ones before are still being written.
    waiting: RefCell<Vec<Change>>,
    writing: Cell<bool>,
    /// Batches sent to the disk lane and not written yet, which quitting waits for.
    unwritten: Arc<AtomicUsize>,
    bookmarks: RefCell<Option<BTreeSet<String>>>,
    /// The past sessions of each folder, newest first, as read last.
    past: RefCell<HashMap<PathBuf, Vec<(Resume, SystemTime)>>>,
    /// Folders whose sessions are being read, and those to read again once that is done.
    reading: RefCell<HashSet<PathBuf>>,
    again: RefCell<HashSet<PathBuf>>,
}

fn store<R>(f: impl FnOnce(&Store) -> R) -> R {
    super::with_state(|state| f(&state.store))
}

/// Records `entry` of a thread, on a subscription or not, with the title the user gave it.
pub(crate) fn record(entry: IndexEntry, subscription: bool, title: Option<String>) {
    change(Change::Record {
        entry,
        subscription,
        title,
    });
}

/// Renames the thread `id` in the index.
pub(crate) fn rename(id: &ThreadId, title: &str) {
    change(Change::Rename(id.clone(), title.to_string()));
}

/// Whether the user bookmarked the thread `id`, as far as the bookmarks were read; they are read on first asking.
pub fn is_bookmarked(id: &ThreadId) -> bool {
    let known = store(|s| s.bookmarks.borrow().as_ref().map(|marks| marks.contains(id.as_str())));
    known.unwrap_or_else(|| {
        read_bookmarks();
        false
    })
}

/// Reads the bookmarks on the disk lane unless they were read.
pub(super) fn read_bookmarks() {
    let unread = store(|s| {
        s.bookmarks.borrow().is_none() && !s.waiting.borrow().iter().any(|c| matches!(c, Change::ReadBookmarks))
    });
    if unread {
        change(Change::ReadBookmarks);
    }
}

/// Bookmarks the thread `id` or takes the bookmark away.
pub(crate) fn set_bookmarked(id: &ThreadId, on: bool) {
    store(|s| {
        if let Some(marks) = s.bookmarks.borrow_mut().as_mut() {
            if on {
                marks.insert(id.as_str().to_string());
            } else {
                marks.remove(id.as_str());
            }
        }
    });
    change(Change::Bookmark(id.clone(), on));
}

/// The sessions of `cwd` that can be resumed, newest first, as read last; reads them again in the background.
pub(crate) fn past_sessions_cached(cwd: &Path, limit: usize) -> Vec<(Resume, SystemTime)> {
    read_past(cwd);
    store(|s| {
        s.past
            .borrow()
            .get(cwd)
            .map(|past| past.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    })
}

/// The session `session` of the folder `root` and the agent that ran it: the agent its thread names, else Claude, whose
/// sessions started in a terminal have no thread. Reads the disk.
pub fn past_session(root: &Path, session: &str) -> Option<(Agent, Resume)> {
    let index = ThreadIndex::open(&ThreadIndex::default_dir());
    let resume = super::find_past_session(&index, root, session)?;
    let agent = Agent::of_thread(&agent_id_of(&index, &resume));
    Some((agent, resume))
}

/// The id of the agent that ran `resume`: the one its thread in `index` names, else Claude's, whose sessions started in
/// a terminal have no thread.
pub(crate) fn agent_id_of(index: &ThreadIndex, resume: &Resume) -> String {
    resume
        .thread
        .as_ref()
        .and_then(|id| index.get(id))
        .map(|entry| entry.agent.clone())
        .or_else(|| resume.agent.clone())
        .unwrap_or_else(|| term_claude::AGENT.to_string())
}

/// Reads the past sessions of `cwd` in the background, or once more after the reading under way.
pub(crate) fn read_past(cwd: &Path) {
    let busy = store(|s| {
        if s.reading.borrow().contains(cwd) {
            s.again.borrow_mut().insert(cwd.to_path_buf());
            return true;
        }
        s.reading.borrow_mut().insert(cwd.to_path_buf());
        false
    });
    if busy {
        return;
    }
    let folder = cwd.to_path_buf();
    let read = move || {
        let past = super::past_sessions(&folder, KEPT);
        main_soon(move || {
            let again = store(|s| {
                s.reading.borrow_mut().remove(&folder);
                s.past.borrow_mut().insert(folder.clone(), past);
                s.again.borrow_mut().remove(&folder)
            });
            if again {
                read_past(&folder);
            }
        });
    };
    if crate::common::util::spawn("agent-past", read).is_err() {
        store(|s| s.reading.borrow_mut().remove(cwd));
    }
}

/// Waits until the changes made so far are on disk, or `limit` passed, as the app quits.
pub fn wait_for_writes(limit: Duration) {
    store(|s| s.writing.set(false));
    send();
    let unwritten = store(|s| s.unwritten.clone());
    let until = Instant::now() + limit;
    while unwritten.load(Ordering::SeqCst) > 0 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn change(change: Change) {
    store(|s| {
        let mut waiting = s.waiting.borrow_mut();
        let replaces = match &change {
            Change::Record { entry, .. } => waiting
                .iter()
                .rposition(|c| c.thread() == Some(&entry.id))
                .filter(|&n| matches!(waiting[n], Change::Record { .. })),
            _ => None,
        };
        match replaces {
            Some(n) => waiting[n] = change,
            None => waiting.push(change),
        }
    });
    send();
}

/// Sends the waiting changes to the disk lane unless a batch is being written, which sends them once it is done.
fn send() {
    let Some((changes, unwritten)) = store(|s| {
        if s.writing.get() || s.waiting.borrow().is_empty() {
            return None;
        }
        s.writing.set(true);
        s.unwritten.fetch_add(1, Ordering::SeqCst);
        Some((std::mem::take(&mut *s.waiting.borrow_mut()), s.unwritten.clone()))
    }) else {
        return;
    };
    // The lane drops a job it has no room for, so the changes stay reachable to write them here then.
    let job = Arc::new(Mutex::new(Some(changes)));
    let queued = job.clone();
    let counted = unwritten.clone();
    let sent = term_base::lane::DISK.try_run(move || {
        let changes = queued.lock_ignoring_poison().take().unwrap_or_default();
        let marks = term_base::lane::contained(|| apply(changes)).flatten();
        counted.fetch_sub(1, Ordering::SeqCst);
        main_soon(move || written(marks));
    });
    if sent.is_err() {
        let changes = job.lock_ignoring_poison().take().unwrap_or_default();
        let marks = apply(changes);
        unwritten.fetch_sub(1, Ordering::SeqCst);
        written(marks);
    }
}

/// Takes in a written batch and the bookmarks it read: the panels and the folders' sessions are read again.
fn written(marks: Option<BTreeSet<String>>) {
    let (first, folders) = store(|s| {
        s.writing.set(false);
        let mut first = false;
        let marked_since = s.waiting.borrow().iter().any(|c| matches!(c, Change::Bookmark(..)));
        if let Some(marks) = marks.filter(|_| !marked_since) {
            first = s.bookmarks.replace(Some(marks)).is_none();
        }
        (first, s.past.borrow().keys().cloned().collect::<Vec<_>>())
    });
    if first {
        for tab in crate::all() {
            tab.bookmarks_read();
        }
    }
    super::reload_all();
    for folder in folders {
        read_past(&folder);
    }
    send();
}

/// Writes `changes` in their order; the bookmarks after them, if any of them read or wrote those.
fn apply(changes: Vec<Change>) -> Option<BTreeSet<String>> {
    let mut index: Option<ThreadIndex> = None;
    let mut marks: Option<BTreeSet<String>> = None;
    for change in changes {
        match change {
            Change::Record {
                entry,
                subscription,
                title,
            } => {
                let index = index_of(&mut index);
                let id = entry.id.clone();
                let _ = index.record_entry(entry, subscription);
                if let Some(title) = title {
                    let _ = index.rename(&id, &title);
                }
            }
            Change::Rename(id, title) => {
                let _ = index_of(&mut index).rename(&id, &title);
            }
            Change::Bookmark(id, on) => {
                let marks = marks.get_or_insert_with(super::bookmarks);
                if on {
                    marks.insert(id.as_str().to_string());
                } else {
                    marks.remove(id.as_str());
                }
                if let Err(error) = super::write_bookmarks(marks) {
                    term_base::diag::report("bookmarks", error);
                }
            }
            Change::ReadBookmarks => {
                marks.get_or_insert_with(super::bookmarks);
            }
        }
    }
    marks
}

/// The index opened once for a batch.
fn index_of(index: &mut Option<ThreadIndex>) -> &mut ThreadIndex {
    index.get_or_insert_with(|| ThreadIndex::open(&ThreadIndex::default_dir()))
}
