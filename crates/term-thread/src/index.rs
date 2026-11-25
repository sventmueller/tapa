//! The list of threads the AI panel's session manager shows: titles, groups, archive and search, kept on disk.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Status, ThreadId};
use crate::thread::Thread;

const FILE: &str = "index.json";
/// The file whose lock lets one index at a time read and write the index file.
const LOCK: &str = "index.lock";
const VERSION: u64 = 1;

/// One thread as the session manager lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(missing_docs, reason = "the names say it")]
pub struct IndexEntry {
    /// The app's id for the thread.
    pub id: ThreadId,
    pub title: String,
    /// The agent's id in the register.
    pub agent: String,
    /// The group the user dragged it into.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// When something last happened in it.
    #[serde(with = "millis")]
    pub last_activity: SystemTime,
    /// Its status when it was last recorded.
    #[serde(default)]
    pub status: Status,
    /// The agent's own id for the conversation, which resuming names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The folder the agent works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// What the session cost, in US dollars, as the agent last reported it; none on a subscription (E57).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl IndexEntry {
    /// The entry for `thread` as it is now.
    pub fn of(thread: &Thread, last_activity: SystemTime) -> IndexEntry {
        IndexEntry {
            id: thread.id().clone(),
            title: thread.title().to_string(),
            agent: thread.agent().to_string(),
            group: None,
            archived: false,
            last_activity,
            status: thread.status(),
            session: thread.session().map(str::to_string),
            cwd: Some(thread.cwd().to_path_buf()),
            cost_usd: thread.usage().billed_cost(),
        }
    }
}

/// Which entries a listing shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Archived entries instead of the others.
    pub archived: bool,
    /// Only this group.
    pub group: Option<String>,
    /// Only entries whose title or agent contains these words, ignoring case.
    pub query: String,
}

/// The index file in a folder; a file written by a newer version or not readable as an index is read as far as
/// possible and never overwritten.
#[derive(Debug, Clone)]
pub struct ThreadIndex {
    path: PathBuf,
    entries: Vec<IndexEntry>,
    read_only: bool,
}

impl ThreadIndex {
    /// The app's folder for it: `threads` in the data folder.
    pub fn default_dir() -> PathBuf {
        term_base::data_dir().join("threads")
    }

    /// The index in `dir`; empty if there is none yet.
    pub fn open(dir: &Path) -> ThreadIndex {
        let path = dir.join(FILE);
        let (entries, read_only) = read(&path);
        ThreadIndex {
            path,
            entries,
            read_only,
        }
    }

    /// Whether changes stay in memory because the file must not be overwritten.
    #[cfg(test)]
    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// The entry of `id`.
    pub fn get(&self, id: &ThreadId) -> Option<&IndexEntry> {
        self.entries.iter().find(|e| &e.id == id)
    }

    /// The entries `filter` lets through, most recent first.
    pub fn list(&self, filter: &Filter) -> Vec<&IndexEntry> {
        let words: Vec<String> = filter.query.split_whitespace().map(str::to_lowercase).collect();
        let mut found: Vec<&IndexEntry> = self
            .entries
            .iter()
            .filter(|e| e.archived == filter.archived)
            .filter(|e| filter.group.is_none() || e.group == filter.group)
            .filter(|e| {
                let haystack = format!("{} {}", e.title, e.agent).to_lowercase();
                words.iter().all(|w| haystack.contains(w.as_str()))
            })
            .collect();
        found.sort_by_key(|e| std::cmp::Reverse(e.last_activity));
        found
    }

    /// The groups in use, sorted.
    #[cfg(test)]
    pub(crate) fn groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self.entries.iter().filter_map(|e| e.group.clone()).collect();
        groups.sort();
        groups.dedup();
        groups
    }

    /// Records `thread` as it is now, keeping its group and archive flag, and writes the file.
    pub fn record(&mut self, thread: &Thread, at: SystemTime) -> io::Result<()> {
        self.record_entry(IndexEntry::of(thread, at), thread.usage().subscription)
    }

    /// `record` for the entry a thread made, on a subscription or not, so the thread itself need not come along.
    pub fn record_entry(&mut self, mut entry: IndexEntry, subscription: bool) -> io::Result<()> {
        self.update(|entries| {
            if let Some(known) = entries.iter().find(|e| e.id == entry.id) {
                entry.group.clone_from(&known.group);
                entry.archived = known.archived;
                if entry.title.is_empty() {
                    entry.title.clone_from(&known.title);
                }
                if entry.cost_usd.is_none() && !subscription {
                    entry.cost_usd = known.cost_usd;
                }
            }
            put(entries, entry);
            Ok(true)
        })
    }

    /// Adds `entry` or replaces the one with its id, and writes the file.
    pub fn upsert(&mut self, entry: IndexEntry) -> io::Result<()> {
        self.update(|entries| {
            put(entries, entry);
            Ok(true)
        })
    }

    /// Renames the thread `id`.
    pub fn rename(&mut self, id: &ThreadId, title: &str) -> io::Result<()> {
        self.change(id, |e| e.title = title.to_string())
    }

    /// Moves the thread `id` into `group`, or out of any.
    #[cfg(test)]
    pub(crate) fn set_group(&mut self, id: &ThreadId, group: Option<&str>) -> io::Result<()> {
        self.change(id, |e| e.group = group.map(str::to_string))
    }

    /// Archives the thread `id` or brings it back.
    pub fn set_archived(&mut self, id: &ThreadId, archived: bool) -> io::Result<()> {
        self.change(id, |e| e.archived = archived)
    }

    /// Forgets the thread `id`.
    pub fn remove(&mut self, id: &ThreadId) -> io::Result<()> {
        self.update(|entries| {
            let before = entries.len();
            entries.retain(|e| &e.id != id);
            Ok(entries.len() != before)
        })
    }

    fn change(&mut self, id: &ThreadId, f: impl FnOnce(&mut IndexEntry)) -> io::Result<()> {
        self.update(|entries| {
            let Some(entry) = entries.iter_mut().find(|e| &e.id == id) else {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("no thread {id}")));
            };
            f(entry);
            Ok(true)
        })
    }

    /// Applies `change` to the entries as the file holds them now and writes them when it says they changed, all
    /// under the folder's lock, so that another index of the same folder, in this process or another, loses nothing.
    fn update(&mut self, change: impl FnOnce(&mut Vec<IndexEntry>) -> io::Result<bool>) -> io::Result<()> {
        if self.read_only {
            return Err(self.stays());
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(LOCK))?;
        lock.lock()?;
        let (mut entries, read_only) = read(&self.path);
        if read_only {
            self.entries = entries;
            self.read_only = true;
            return Err(self.stays());
        }
        let changed = change(&mut entries);
        let written = match changed {
            Ok(true) => {
                let document = serde_json::json!({ "version": VERSION, "threads": entries });
                term_base::atomic_write::write_json(&self.path, &document)
            }
            Ok(false) => Ok(()),
            Err(error) => Err(error),
        };
        if written.is_ok() {
            self.entries = entries;
        }
        written
    }

    fn stays(&self) -> io::Error {
        io::Error::other(format!("{} stays as it is", self.path.display()))
    }
}

/// Adds `entry` to `entries` or replaces the one with its id.
fn put(entries: &mut Vec<IndexEntry>, entry: IndexEntry) {
    match entries.iter_mut().find(|e| e.id == entry.id) {
        Some(known) => *known = entry,
        None => entries.push(entry),
    }
}

/// The entries of the index file at `path`, and whether it must not be overwritten: written by a newer version, or
/// not readable as an index.
fn read(path: &Path) -> (Vec<IndexEntry>, bool) {
    match std::fs::read(path) {
        Err(_) => (Vec::new(), false),
        Ok(bytes) if term_base::atomic_write::is_torn(&bytes) => (Vec::new(), false),
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => {
                let newer = map.get("version").and_then(Value::as_u64).is_some_and(|v| v > VERSION);
                let entries = map
                    .get("threads")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(|v| IndexEntry::deserialize(v).ok()).collect())
                    .unwrap_or_default();
                (entries, newer)
            }
            _ => (Vec::new(), true),
        },
    }
}

mod millis {
    use super::{Duration, SystemTime, UNIX_EPOCH};
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error> {
        let millis = time.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
        serializer.serialize_u64(u64::try_from(millis).unwrap_or(u64::MAX))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SystemTime, D::Error> {
        Ok(UNIX_EPOCH + Duration::from_millis(u64::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_base::testing::scratch;

    fn entry(id: &str, title: &str, seconds: u64) -> IndexEntry {
        IndexEntry {
            id: ThreadId::from(id),
            title: title.into(),
            agent: "claude".into(),
            group: None,
            archived: false,
            last_activity: UNIX_EPOCH + Duration::from_secs(seconds),
            status: Status::Idle,
            session: None,
            cwd: None,
            cost_usd: None,
        }
    }

    #[test]
    fn entries_survive_a_reopen_most_recent_first() {
        let dir = scratch("reopen");
        let mut index = ThreadIndex::open(&dir);
        index.upsert(entry("a", "Fix the parser", 10)).unwrap();
        index.upsert(entry("b", "Write tests", 20)).unwrap();
        let again = ThreadIndex::open(&dir);
        let titles: Vec<&str> = again
            .list(&Filter::default())
            .iter()
            .map(|e| e.title.as_str())
            .collect();
        assert_eq!(titles, ["Write tests", "Fix the parser"]);
    }

    #[test]
    fn search_groups_and_archive_narrow_the_list() {
        let dir = scratch("filter");
        let mut index = ThreadIndex::open(&dir);
        index.upsert(entry("a", "Fix the Parser", 10)).unwrap();
        index.upsert(entry("b", "Write tests", 20)).unwrap();
        index.upsert(entry("c", "Parser docs", 30)).unwrap();
        index.set_group(&ThreadId::from("a"), Some("Work")).unwrap();
        index.set_archived(&ThreadId::from("c"), true).unwrap();
        let query = Filter {
            query: "parser".into(),
            ..Filter::default()
        };
        assert_eq!(index.list(&query).len(), 1);
        let archived = Filter {
            archived: true,
            ..Filter::default()
        };
        assert_eq!(index.list(&archived)[0].title, "Parser docs");
        let grouped = Filter {
            group: Some("Work".into()),
            ..Filter::default()
        };
        assert_eq!(index.list(&grouped)[0].id, ThreadId::from("a"));
        assert_eq!(index.groups(), ["Work"]);
    }

    #[test]
    fn recording_a_thread_keeps_its_group_and_archive_flag() {
        let dir = scratch("record");
        let mut index = ThreadIndex::open(&dir);
        let mut thread = Thread::new(ThreadId::from("t"), "claude", "/tmp");
        thread.apply(crate::Update::Title("First".into()));
        index.record(&thread, UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        index.set_group(&thread.id().clone(), Some("G")).unwrap();
        thread.apply(crate::Update::Title("Second".into()));
        index.record(&thread, UNIX_EPOCH + Duration::from_secs(2)).unwrap();
        let known = index.get(thread.id()).unwrap();
        assert_eq!(known.title, "Second");
        assert_eq!(known.group.as_deref(), Some("G"));
    }

    #[test]
    fn the_session_cost_is_kept_until_a_new_one_is_reported() {
        let dir = scratch("cost");
        let mut index = ThreadIndex::open(&dir);
        let mut thread = Thread::new(ThreadId::from("t"), "claude", "/tmp");
        thread.apply(crate::Update::Usage(crate::UsagePatch {
            cost_usd: Some(0.25),
            ..crate::UsagePatch::default()
        }));
        index.record(&thread, UNIX_EPOCH + Duration::from_secs(1)).unwrap();
        let reopened = Thread::new(ThreadId::from("t"), "claude", "/tmp");
        let mut again = ThreadIndex::open(&dir);
        again.record(&reopened, UNIX_EPOCH + Duration::from_secs(2)).unwrap();
        assert_eq!(
            ThreadIndex::open(&dir).get(thread.id()).unwrap().cost_usd,
            Some(0.25),
            "a thread without a cost yet, such as one opened again, keeps the recorded cost"
        );
        let mut paid_by_plan = Thread::new(ThreadId::from("t"), "claude", "/tmp");
        paid_by_plan.apply(crate::Update::Usage(crate::UsagePatch {
            cost_usd: Some(0.5),
            subscription: Some(true),
            ..crate::UsagePatch::default()
        }));
        again
            .record(&paid_by_plan, UNIX_EPOCH + Duration::from_secs(3))
            .unwrap();
        assert_eq!(
            ThreadIndex::open(&dir).get(thread.id()).unwrap().cost_usd,
            None,
            "on a subscription the list shows no cost"
        );
    }

    #[test]
    fn a_file_from_a_newer_version_is_read_but_not_overwritten() {
        let dir = scratch("newer");
        std::fs::create_dir_all(&dir).unwrap();
        let original =
            r#"{"version": 7, "threads": [{"id": "x", "title": "Kept", "agent": "claude", "last_activity": 5}]}"#;
        std::fs::write(dir.join(FILE), original).unwrap();
        let mut index = ThreadIndex::open(&dir);
        assert!(index.is_read_only());
        assert_eq!(index.get(&ThreadId::from("x")).unwrap().title, "Kept");
        assert!(index.rename(&ThreadId::from("x"), "Changed").is_err());
        assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), original);
    }

    #[test]
    fn an_empty_file_left_by_a_power_loss_is_written_again() {
        let dir = scratch("torn");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), "").unwrap();
        let mut index = ThreadIndex::open(&dir);
        assert!(!index.is_read_only());
        index.upsert(entry("a", "After the power loss", 1)).unwrap();
        assert_eq!(ThreadIndex::open(&dir).list(&Filter::default()).len(), 1);
    }

    #[test]
    fn two_indexes_saving_at_once_leave_a_whole_file_with_both_lists() {
        let dir = scratch("two-writers");
        let writers: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| {
                let dir = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    let mut index = ThreadIndex::open(&dir);
                    for n in 0..30 {
                        index.upsert(entry(&format!("{name}{n}"), "A title", n)).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let index = ThreadIndex::open(&dir);
        assert!(!index.is_read_only(), "the file is whole");
        assert_eq!(
            index.list(&Filter::default()).len(),
            60,
            "both writers' lists, all of them"
        );
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, [FILE, LOCK], "no scratch file left");
    }

    #[test]
    fn an_index_opened_earlier_keeps_what_another_wrote_since() {
        let dir = scratch("stale");
        let mut first = ThreadIndex::open(&dir);
        let mut second = ThreadIndex::open(&dir);
        first.upsert(entry("a", "First's", 1)).unwrap();
        second.upsert(entry("b", "Second's", 2)).unwrap();
        second.rename(&ThreadId::from("a"), "Renamed by the second").unwrap();
        first.remove(&ThreadId::from("gone")).unwrap();
        first.set_archived(&ThreadId::from("b"), true).unwrap();
        let again = ThreadIndex::open(&dir);
        assert_eq!(again.get(&ThreadId::from("a")).unwrap().title, "Renamed by the second");
        assert!(again.get(&ThreadId::from("b")).unwrap().archived);
        assert_eq!(
            first.get(&ThreadId::from("b")).unwrap().title,
            "Second's",
            "a change reads the file first"
        );
    }

    #[test]
    fn a_damaged_entry_is_skipped_and_the_rest_is_read() {
        let dir = scratch("damaged");
        std::fs::create_dir_all(&dir).unwrap();
        let text =
            r#"{"version": 1, "threads": [{"id": 3}, {"id": "ok", "title": "T", "agent": "a", "last_activity": 1}]}"#;
        std::fs::write(dir.join(FILE), text).unwrap();
        let index = ThreadIndex::open(&dir);
        assert!(!index.is_read_only());
        assert_eq!(index.list(&Filter::default()).len(), 1);
    }
}
