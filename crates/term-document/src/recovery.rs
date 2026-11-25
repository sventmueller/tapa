//! Copies of unsaved documents, untitled ones included, that a restart after a crash brings back. Each copy is the
//! document's text, the text its file held then, so a file changed meanwhile can be merged, and a small description;
//! the description is written last. A text without one, left by a crash during the first copy of a document, comes back
//! as an untitled document, since nothing says where it belongs. Copies that cannot be read go to `damaged/` beside the
//! others, where they stay for a person to look at.
//!
//! Every file of a copy is sealed with the key of the app's restored state (E17), so neither text nor path lies on disk
//! in plain text; a file sealed with another key counts as damaged.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use splat_file_state::{Encoding, FileFormat};
use term_seal::KEY_LEN;

use crate::disk;
use crate::document::{Document, Snapshot};

const DESCRIPTION: &str = "json";
const TEXT: &str = "text";
const BASE: &str = "base";
const DAMAGED: &str = "damaged";
const FORMAT_VERSION: u32 = 1;

/// Names the copy of one document; it stays the same while the document is open, so each copy replaces the last.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecoveryId(String);

impl Default for RecoveryId {
    fn default() -> RecoveryId {
        RecoveryId::new()
    }
}

impl RecoveryId {
    /// A name no other copy has, in this run or another.
    pub fn new() -> RecoveryId {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos());
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        RecoveryId(format!("{nanos:x}-{:x}-{n:x}", std::process::id()))
    }

    /// The name as it appears in the copy's file names.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name `name` as `as_str` gave it, if it is one.
    pub fn parse(name: &str) -> Option<RecoveryId> {
        let valid = !name.is_empty() && name.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        valid.then(|| RecoveryId(name.to_string()))
    }
}

#[derive(Serialize, Deserialize)]
struct Description {
    version: u32,
    path: Option<PathBuf>,
    title: Option<String>,
    encoding: String,
    bom: bool,
    revision: u64,
    saved: u64,
    base: Option<u64>,
    #[serde(default)]
    project: Option<PathBuf>,
}

/// A copy that can be brought back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The copy's name.
    pub id: RecoveryId,
    /// The document's file, if it had one.
    pub path: Option<PathBuf>,
    /// What the window called an untitled document.
    pub title: Option<String>,
    /// The folder of the window that showed the document.
    pub project: Option<PathBuf>,
    /// When the copy was made.
    pub saved: SystemTime,
}

/// A copy read back, to become a document again.
#[derive(Debug, Clone)]
pub struct Recovered {
    /// What the copy is.
    pub pending: Pending,
    text: String,
    base: Option<String>,
    format: FileFormat,
}

impl Recovered {
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The document as it was, unsaved; a look at its file then merges what changed there meanwhile.
    pub fn into_document(self) -> Document {
        Document::recovered(&self.text, self.pending.path, self.format, self.base.as_deref())
    }
}

/// The folder of the copies and the key they are sealed with.
#[derive(Clone)]
pub struct Recovery {
    dir: PathBuf,
    key: [u8; KEY_LEN],
}

impl std::fmt::Debug for Recovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recovery")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Recovery {
    /// Copies in `dir`, sealed with `key`.
    pub fn new(dir: PathBuf, key: [u8; KEY_LEN]) -> Recovery {
        Recovery { dir, key }
    }

    /// Copies in the app's data folder, `recovery/` under `term_base::data_dir()`, sealed with `key`, which is
    /// `term_seal::key()` outside a self-test.
    pub fn in_data_dir(key: [u8; KEY_LEN]) -> Recovery {
        Recovery::new(term_base::data_dir().join("recovery"), key)
    }

    /// The folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, id: &RecoveryId, kind: &str) -> PathBuf {
        self.dir.join(format!("{}.{kind}", id.0))
    }

    fn write(&self, id: &RecoveryId, kind: &str, plain: &[u8]) -> io::Result<()> {
        let sealed = term_seal::seal(&self.key, plain).map_err(io::Error::other)?;
        disk::write(&self.file(id, kind), &sealed).map(|_| ())
    }

    fn read(&self, id: &RecoveryId, kind: &str) -> io::Result<Vec<u8>> {
        let sealed = fs::read(self.file(id, kind))?;
        term_seal::open(&self.key, &sealed).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn read_text(&self, id: &RecoveryId, kind: &str) -> io::Result<String> {
        String::from_utf8(self.read(id, kind)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Stores `snapshot` as the copy `id`, with `title` for an untitled document and the folder of the window that
    /// shows it. The text the file held is written only when it differs from the one the copy already has.
    pub fn store(
        &self,
        id: &RecoveryId,
        snapshot: &Snapshot,
        title: Option<&str>,
        project: Option<&Path>,
    ) -> io::Result<()> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&self.dir)?;
        self.write(id, TEXT, snapshot.text.to_string().as_bytes())?;
        let stored_base = self.description(id).ok().and_then(|description| description.base);
        let base = match &snapshot.base {
            Some((text, generation)) if stored_base != Some(*generation) || !self.file(id, BASE).exists() => {
                self.write(id, BASE, text.to_string().as_bytes())?;
                Some(*generation)
            }
            Some((_, generation)) => Some(*generation),
            None => {
                remove_if_there(&self.file(id, BASE))?;
                None
            }
        };
        let description = Description {
            version: FORMAT_VERSION,
            path: snapshot.path.clone(),
            title: title.map(str::to_string),
            encoding: encoding_name(snapshot.format.encoding).to_string(),
            bom: snapshot.format.bom,
            revision: snapshot.revision.get(),
            saved: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs()),
            base,
            project: project.map(Path::to_path_buf),
        };
        let json = serde_json::to_vec_pretty(&description).map_err(io::Error::other)?;
        self.write(id, DESCRIPTION, &json)
    }

    fn description(&self, id: &RecoveryId) -> io::Result<Description> {
        let json = self.read(id, DESCRIPTION)?;
        serde_json::from_slice(&json).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// The copies that can be brought back, newest first: after a crash, what was unsaved when it happened.
    pub fn pending(&self) -> io::Result<Vec<Pending>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(RecoveryId::parse) else {
                continue;
            };
            match path.extension().and_then(|e| e.to_str()) {
                Some(DESCRIPTION) => {
                    let Ok(description) = self.description(&id) else {
                        continue;
                    };
                    if description.version <= FORMAT_VERSION && self.file(&id, TEXT).exists() {
                        found.push(pending(id, &description));
                    }
                }
                Some(TEXT) if !self.file(&id, DESCRIPTION).exists() => found.push(self.orphan(id)),
                _ => {}
            }
        }
        found.sort_by(|a, b| b.saved.cmp(&a.saved).then_with(|| a.id.cmp(&b.id)));
        Ok(found)
    }

    /// A text without a description, as an untitled document of the time it was written.
    fn orphan(&self, id: RecoveryId) -> Pending {
        let saved = fs::metadata(self.file(&id, TEXT))
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);
        Pending {
            id,
            path: None,
            title: None,
            project: None,
            saved,
        }
    }

    /// Reads the copy `id` back; a text without a description comes back untitled, if it is UTF-8.
    pub fn restore(&self, id: &RecoveryId) -> io::Result<Recovered> {
        if !self.file(id, DESCRIPTION).exists() {
            let text = self.read_text(id, TEXT)?;
            return Ok(Recovered {
                pending: self.orphan(id.clone()),
                text,
                base: None,
                format: FileFormat::default(),
            });
        }
        let description = self.description(id)?;
        let text = self.read_text(id, TEXT)?;
        let base = match description.base {
            Some(_) => Some(self.read_text(id, BASE)?),
            None => None,
        };
        let format = FileFormat {
            bom: description.bom,
            crlf: false,
            encoding: encoding_named(&description.encoding),
        };
        Ok(Recovered {
            pending: pending(id.clone(), &description),
            text,
            base,
            format,
        })
    }

    /// The copies that cannot be read: a description that is no JSON of this format or has no text beside it, and a
    /// base with neither. A copy written by a newer version of the app is not among them, nor a text without a
    /// description, which comes back untitled.
    pub fn damaged(&self) -> io::Result<Vec<RecoveryId>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).and_then(RecoveryId::parse) else {
                continue;
            };
            let kind = path.extension().and_then(|e| e.to_str());
            let broken = match kind {
                Some(DESCRIPTION) => match self.description(&id) {
                    Ok(description) => description.version <= FORMAT_VERSION && !self.file(&id, TEXT).exists(),
                    Err(_) => true,
                },
                Some(BASE) => !self.file(&id, DESCRIPTION).exists() && !self.file(&id, TEXT).exists(),
                _ => false,
            };
            if broken && !found.contains(&id) {
                found.push(id);
            }
        }
        found.sort();
        Ok(found)
    }

    /// Moves the files of the copy `id` to `damaged/`, out of the way of every later look but kept.
    pub fn set_aside(&self, id: &RecoveryId) -> io::Result<()> {
        let aside = self.dir.join(DAMAGED);
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&aside)?;
        for kind in [DESCRIPTION, TEXT, BASE] {
            let from = self.file(id, kind);
            match fs::rename(&from, aside.join(format!("{}.{kind}", id.0))) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
    }

    /// Removes the copy `id`, as after a save or once the user declines to bring it back.
    pub fn discard(&self, id: &RecoveryId) -> io::Result<()> {
        remove_if_there(&self.file(id, DESCRIPTION))?;
        remove_if_there(&self.file(id, TEXT))?;
        remove_if_there(&self.file(id, BASE))
    }
}

fn pending(id: RecoveryId, description: &Description) -> Pending {
    Pending {
        id,
        path: description.path.clone(),
        title: description.title.clone(),
        project: description.project.clone(),
        saved: UNIX_EPOCH + Duration::from_secs(description.saved),
    }
}

fn remove_if_there(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn encoding_name(encoding: Encoding) -> &'static str {
    match encoding {
        Encoding::Utf8 => "utf-8",
        Encoding::Utf16Le => "utf-16le",
        Encoding::Utf16Be => "utf-16be",
    }
}

fn encoding_named(name: &str) -> Encoding {
    match name {
        "utf-16le" => Encoding::Utf16Le,
        "utf-16be" => Encoding::Utf16Be,
        _ => Encoding::Utf8,
    }
}

/// When to store each document's copy: once its changes rest for `quiet`, and at the latest `longest` after the
/// first change not yet stored, so that steady typing is stored as well.
#[derive(Debug, Clone)]
pub struct Debounce {
    quiet: Duration,
    longest: Duration,
    waiting: HashMap<RecoveryId, (Instant, Instant)>,
}

impl Debounce {
    /// Copies after `quiet` without changes, or `longest` after the first unstored change.
    pub fn new(quiet: Duration, longest: Duration) -> Debounce {
        Debounce {
            quiet,
            longest,
            waiting: HashMap::new(),
        }
    }

    /// The document of `id` changed at `now`.
    pub fn changed(&mut self, id: &RecoveryId, now: Instant) {
        self.waiting
            .entry(id.clone())
            .and_modify(|(_, last)| *last = now)
            .or_insert((now, now));
    }

    fn due_at(&self, (first, last): (Instant, Instant)) -> Instant {
        (last + self.quiet).min(first + self.longest)
    }

    /// The copies to store at `now`; they wait no longer.
    pub fn due(&mut self, now: Instant) -> Vec<RecoveryId> {
        let mut due: Vec<RecoveryId> = self
            .waiting
            .iter()
            .filter(|(_, times)| self.due_at(**times) <= now)
            .map(|(id, _)| id.clone())
            .collect();
        due.sort();
        for id in &due {
            self.waiting.remove(id);
        }
        due
    }

    /// When the next copy is due, to set a timer for.
    pub fn next_due(&self) -> Option<Instant> {
        self.waiting.values().map(|times| self.due_at(*times)).min()
    }

    /// The document of `id` was saved or closed; its copy need not be stored.
    pub fn forget(&mut self, id: &RecoveryId) {
        self.waiting.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::tests::scratch;
    use crate::edit::Edit;

    const KEY: [u8; KEY_LEN] = [5; KEY_LEN];

    fn sealed_in(dir: PathBuf) -> Recovery {
        Recovery::new(dir, KEY)
    }

    #[test]
    fn an_untitled_document_comes_back_with_its_text_and_title() {
        let dir = scratch("recovery-untitled");
        let recovery = sealed_in(dir.join("recovery"));
        let id = RecoveryId::new();
        let document = Document::untitled("Entwurf\r\nzwei");
        recovery
            .store(
                &id,
                &document.snapshot(),
                Some("Untitled 3"),
                Some(Path::new("/projects/p")),
            )
            .unwrap();
        let pending = recovery.pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            (pending[0].id.clone(), pending[0].title.as_deref()),
            (id.clone(), Some("Untitled 3"))
        );
        let back = recovery.restore(&id).unwrap().into_document();
        assert_eq!(back.text().to_string(), "Entwurf\r\nzwei");
        assert!(back.is_dirty() && back.path().is_none());
        recovery.discard(&id).unwrap();
        assert!(recovery.pending().unwrap().is_empty());
        assert_eq!(fs::read_dir(recovery.dir()).unwrap().count(), 0);
    }

    #[test]
    fn no_file_of_a_copy_holds_its_text_path_or_title_in_plain_text() {
        let dir = scratch("recovery-sealed");
        let path = dir.join("geheime-notizen.txt");
        fs::write(&path, "der alte Stand der Datei\n").unwrap();
        let mut document = Document::open(&path).unwrap();
        document
            .apply(Edit {
                range_utf16: 0..0,
                text: "Kennwort hunter2 ".into(),
            })
            .unwrap();
        let recovery = sealed_in(dir.join("recovery"));
        let id = RecoveryId::new();
        recovery
            .store(
                &id,
                &document.snapshot(),
                Some("Notizen"),
                Some(Path::new("/projekte/geheim")),
            )
            .unwrap();
        let mut files = 0;
        for entry in fs::read_dir(recovery.dir()).unwrap() {
            let bytes = fs::read(entry.unwrap().path()).unwrap();
            files += 1;
            for plain in [
                "hunter2",
                "der alte Stand",
                "geheime-notizen",
                "Notizen",
                "/projekte/geheim",
                "version",
            ] {
                assert!(
                    !bytes.windows(plain.len()).any(|part| part == plain.as_bytes()),
                    "{plain} lies on disk in plain text"
                );
            }
        }
        assert_eq!(files, 3, "text, base and description");
        let back = recovery.restore(&id).unwrap();
        assert_eq!(back.text(), "Kennwort hunter2 der alte Stand der Datei\n");
        assert_eq!(back.pending.title.as_deref(), Some("Notizen"));

        let other = Recovery::new(recovery.dir().to_path_buf(), [6; KEY_LEN]);
        assert!(other.pending().unwrap().is_empty(), "another key opens nothing");
        assert_eq!(other.damaged().unwrap(), std::slice::from_ref(&id));
        assert_eq!(
            other.restore(&id).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        assert!(!format!("{recovery:?}").contains("key"), "the key stays out of the log");
    }

    #[test]
    fn a_text_without_its_description_comes_back_untitled_if_it_decodes() {
        let dir = scratch("recovery-partial");
        let recovery = sealed_in(dir.to_path_buf());
        let id = RecoveryId::new();
        let path = recovery.dir().join("x.txt");
        let mut document = Document::untitled("Text der Datei");
        document.set_path(path.clone());
        recovery
            .store(&id, &document.snapshot(), None, Some(Path::new("/p")))
            .unwrap();
        fs::remove_file(recovery.file(&id, DESCRIPTION)).unwrap();
        fs::write(recovery.dir().join("not an id.json"), "{}").unwrap();
        let undecodable = RecoveryId::new();
        recovery.write(&undecodable, TEXT, b"Gr\xFC\xDFe").unwrap();
        let pending = recovery.pending().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(pending.iter().all(|p| p.path.is_none() && p.project.is_none()));
        let back = recovery.restore(&id).unwrap();
        assert_eq!(back.text(), "Text der Datei");
        let document = back.into_document();
        assert!(
            document.path().is_none() && document.is_dirty(),
            "nothing says which file it was"
        );
        assert!(recovery.restore(&undecodable).is_err());
        assert!(recovery.damaged().unwrap().is_empty());
        assert!(
            sealed_in(scratch("recovery-none").join("missing"))
                .pending()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn damaged_copies_are_set_aside_and_kept_and_newer_ones_left_alone() {
        let dir = scratch("recovery-damaged");
        let recovery = sealed_in(dir.to_path_buf());
        let store = |text: &str| {
            let id = RecoveryId::new();
            recovery
                .store(&id, &Document::untitled(text).snapshot(), None, None)
                .unwrap();
            id
        };
        let good = store("gut");
        let garbled = store("x");
        fs::write(recovery.file(&garbled, DESCRIPTION), "{ kein JSON").unwrap();
        let newer = store("neu");
        let description = String::from_utf8(recovery.read(&newer, DESCRIPTION).unwrap()).unwrap();
        let newer_description = description.replace("\"version\": 1", "\"version\": 99");
        recovery
            .write(&newer, DESCRIPTION, newer_description.as_bytes())
            .unwrap();
        let orphan = RecoveryId::new();
        fs::write(recovery.file(&orphan, BASE), "nur der Text der Datei").unwrap();
        let mut expected = vec![garbled.clone(), orphan.clone()];
        expected.sort();
        assert_eq!(recovery.damaged().unwrap(), expected);
        for id in &expected {
            recovery.set_aside(id).unwrap();
        }
        assert!(recovery.damaged().unwrap().is_empty());
        let pending: Vec<RecoveryId> = recovery.pending().unwrap().into_iter().map(|p| p.id).collect();
        assert_eq!(pending, std::slice::from_ref(&good));
        let aside = recovery.dir().join(DAMAGED);
        assert_eq!(
            fs::read_to_string(aside.join(format!("{}.{DESCRIPTION}", garbled.as_str()))).unwrap(),
            "{ kein JSON"
        );
        assert!(aside.join(format!("{}.{BASE}", orphan.as_str())).exists());
        assert!(recovery.file(&newer, TEXT).exists(), "a newer app may still read it");
        assert_eq!(RecoveryId::parse(good.as_str()), Some(good));
        assert_eq!(RecoveryId::parse("../x"), None);
    }

    #[test]
    fn a_recovered_file_merges_what_changed_on_disk_meanwhile() {
        let dir = scratch("recovery-merge");
        let path = dir.join("notes.txt");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        let mut document = Document::open(&path).unwrap();
        document
            .apply(Edit {
                range_utf16: 0..3,
                text: "ONE".into(),
            })
            .unwrap();
        let recovery = sealed_in(dir.join("recovery"));
        let id = RecoveryId::new();
        recovery.store(&id, &document.snapshot(), None, None).unwrap();
        let first = fs::metadata(recovery.file(&id, BASE)).unwrap().modified().unwrap();
        recovery.store(&id, &document.snapshot(), None, None).unwrap();
        let second = fs::metadata(recovery.file(&id, BASE)).unwrap().modified().unwrap();
        assert_eq!(first, second, "an unchanged base is not written again");
        drop(document);

        fs::write(&path, "one\ntwo\nTHREE\n").unwrap();
        let mut back = recovery.restore(&id).unwrap().into_document();
        assert!(back.is_dirty());
        assert!(
            matches!(back.save(), Err(crate::SaveError::ChangedOnDisk)),
            "a save before the merge would lose the change on disk"
        );
        let check = back.disk_probe().unwrap().check().unwrap();
        let outcome = back.take_disk(check).unwrap();
        assert!(matches!(outcome, crate::Outcome::Merged(_)), "{outcome:?}");
        assert_eq!(back.text().to_string(), "ONE\ntwo\nTHREE\n");
        assert!(back.is_dirty());
    }

    #[test]
    fn a_copy_is_due_after_a_rest_or_after_the_longest_wait() {
        let mut debounce = Debounce::new(Duration::from_secs(2), Duration::from_secs(10));
        let (a, b) = (RecoveryId::new(), RecoveryId::new());
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        for second in 0..12 {
            debounce.changed(&a, at(second));
        }
        debounce.changed(&b, at(5));
        assert_eq!(debounce.next_due(), Some(at(7)));
        assert!(debounce.due(at(6)).is_empty());
        assert_eq!(debounce.due(at(7)), std::slice::from_ref(&b));
        assert!(debounce.due(at(9)).is_empty(), "a is still being typed in");
        assert_eq!(
            debounce.due(at(10)),
            std::slice::from_ref(&a),
            "but not for longer than the longest wait"
        );
        debounce.changed(&b, at(11));
        debounce.forget(&b);
        assert_eq!(debounce.next_due(), None);
    }
}
