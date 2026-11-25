//! The command history: rows in SQLite for the next start, and all of them in memory for the answers.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::rank::{self, Context};

/// The most commands kept in memory; older ones stay in the file.
const IN_MEMORY: usize = 50_000;

/// One command as the shell ran it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The command line.
    pub line: String,
    /// The folder it ran in.
    pub folder: PathBuf,
    /// The branch of the folder's repository.
    pub branch: Option<String>,
    /// The command before it in the same session.
    pub previous: Option<String>,
    /// How it ended.
    pub exit_code: Option<i32>,
    /// When it started, in seconds since 1970.
    pub started: u64,
}

/// The history of every session.
#[derive(Debug)]
pub struct History {
    connection: Option<Connection>,
    entries: Vec<Entry>,
}

/// Now in seconds since 1970.
pub(crate) fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl History {
    /// The history in the SQLite file at `path`, made if it does not exist; an unreadable file gives an empty history
    /// that is only kept in memory.
    pub fn open(path: &Path) -> History {
        let connection = path
            .parent()
            .and_then(|dir| std::fs::create_dir_all(dir).ok())
            .and_then(|()| {
                let connection = Connection::open(path).ok()?;
                connection
                    .execute_batch(
                        "PRAGMA journal_mode = WAL;
                     CREATE TABLE IF NOT EXISTS commands (
                        id INTEGER PRIMARY KEY,
                        line TEXT NOT NULL,
                        folder TEXT NOT NULL,
                        branch TEXT,
                        previous TEXT,
                        exit_code INTEGER,
                        started INTEGER NOT NULL
                     );",
                    )
                    .ok()?;
                Some(connection)
            });
        let entries = connection.as_ref().map(load).unwrap_or_default();
        History { connection, entries }
    }

    /// An empty history in memory only, for tests.
    pub fn in_memory() -> History {
        History {
            connection: None,
            entries: Vec::new(),
        }
    }

    /// Keeps `entry`, in memory at once and in the file.
    pub fn record(&mut self, entry: Entry) {
        if entry.line.trim().is_empty() {
            return;
        }
        if let Some(connection) = &self.connection {
            let _ = connection.execute(
                "INSERT INTO commands (line, folder, branch, previous, exit_code, started) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entry.line,
                    entry.folder.to_string_lossy(),
                    entry.branch,
                    entry.previous,
                    entry.exit_code,
                    i64::try_from(entry.started).unwrap_or(i64::MAX)
                ],
            );
        }
        if self.entries.len() >= IN_MEMORY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    /// The best command starting with `prefix` in `context`, to show after what was typed.
    pub fn suggest(&self, prefix: &str, context: &Context) -> Option<String> {
        if prefix.trim().is_empty() {
            return None;
        }
        rank::best(&self.entries, context, now(), |line| {
            line.starts_with(prefix) && line != prefix
        })
    }

    /// The command most likely to come next in `context` without any input (F210): what followed the previous
    /// command most often, in the same folder before anywhere.
    pub fn next_command(&self, context: &Context) -> Option<String> {
        let previous = context.previous.as_deref()?;
        let followers: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.previous.as_deref() == Some(previous) && entry.exit_code.is_none_or(|c| c == 0))
            .cloned()
            .collect();
        rank::best(&followers, context, now(), |line| line != previous)
    }

    /// The commands containing `needle`, newest first, at most `limit`, as ⌃R lists them.
    pub fn search(&self, needle: &str, limit: usize) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.entries
            .iter()
            .rev()
            .filter(|entry| entry.line.contains(needle))
            .filter(|entry| seen.insert(entry.line.clone()))
            .take(limit)
            .map(|entry| entry.line.clone())
            .collect()
    }

    /// How many commands are in memory.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no command is in memory.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn load(connection: &Connection) -> Vec<Entry> {
    let Ok(mut statement) = connection
        .prepare("SELECT line, folder, branch, previous, exit_code, started FROM commands ORDER BY id DESC LIMIT ?1")
    else {
        return Vec::new();
    };
    let rows = statement.query_map(params![i64::try_from(IN_MEMORY).unwrap_or(i64::MAX)], |row| {
        Ok(Entry {
            line: row.get(0)?,
            folder: PathBuf::from(row.get::<_, String>(1)?),
            branch: row.get(2)?,
            previous: row.get(3)?,
            exit_code: row.get(4)?,
            started: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
        })
    });
    let mut entries: Vec<Entry> = rows.map(|rows| rows.flatten().collect()).unwrap_or_default();
    entries.reverse();
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: &str, folder: &str, previous: Option<&str>, started: u64) -> Entry {
        Entry {
            line: line.into(),
            folder: folder.into(),
            branch: Some("main".into()),
            previous: previous.map(Into::into),
            exit_code: Some(0),
            started,
        }
    }

    #[test]
    fn the_history_survives_a_restart() {
        let dir = term_base::testing::scratch("suggest-history");
        let path = dir.join("history.sqlite");
        let mut history = History::open(&path);
        history.record(entry("cargo test", "/p", None, now()));
        drop(history);
        let history = History::open(&path);
        assert_eq!(history.len(), 1);
        assert_eq!(history.search("cargo", 5), ["cargo test"]);
    }

    #[test]
    fn suggestions_prefer_this_folder_and_the_next_command_follows_the_previous() {
        let mut history = History::in_memory();
        let now = now();
        history.record(entry("git status", "/a", None, now - 100));
        history.record(entry("git stash", "/b", None, now - 50));
        history.record(entry("git add -A", "/a", Some("git status"), now - 40));
        history.record(entry("git commit", "/a", Some("git add -A"), now - 30));
        history.record(entry("git commit", "/a", Some("git add -A"), now - 20));
        let here = Context {
            folder: "/a".into(),
            branch: Some("main".into()),
            previous: Some("git add -A".into()),
            exit_code: Some(0),
        };
        assert_eq!(history.suggest("git st", &here).as_deref(), Some("git status"));
        assert_eq!(history.next_command(&here).as_deref(), Some("git commit"));
        assert_eq!(history.suggest("", &here), None);
        assert_eq!(history.suggest("git status", &here), None, "nothing to add");
    }
}
