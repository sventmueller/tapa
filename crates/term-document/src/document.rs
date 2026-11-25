//! One buffer per file: its text and revision, whether a file holds it, edits, saving and what happens when the file
//! changes on disk.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use splat_file_state::{FileFormat, Fingerprint, Stamp};

use crate::disk::{self, FileContent, ReadOnly};
use crate::edit::{ByteEdit, Change, Edit, EditError, edits_between};
use crate::merge::{self, Conflict, Reconciliation};
#[cfg(test)]
use crate::syntax::BracketPair;
use crate::syntax::{Fold, Language, LineEdit, Span, Styled, Syntax};
use crate::text::{PositionError, Text};

/// A kind of line break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    /// `\n`, as on macOS and Linux.
    #[default]
    Lf,
    /// `\r\n`, as on Windows.
    Crlf,
    /// A lone `\r`, as on classic Mac OS.
    Cr,
}

impl LineEnding {
    /// The characters of the break.
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }
}

/// The line breaks of a text: every line keeps its own, and a new line gets the one most lines have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LineEndings {
    /// The break most lines end in, which a new line gets; `Lf` in a text without breaks.
    pub dominant: LineEnding,
    /// Whether lines end in more than one kind of break.
    pub(crate) mixed: bool,
}

impl LineEndings {
    fn of(text: &Text) -> LineEndings {
        let (mut lf, mut crlf, mut cr) = (0usize, 0usize, 0usize);
        let mut after_cr = false;
        for chunk in text.chunks() {
            for byte in chunk.bytes() {
                match byte {
                    b'\n' if after_cr => {
                        crlf += 1;
                        cr -= 1;
                    }
                    b'\n' => lf += 1,
                    b'\r' => cr += 1,
                    _ => {}
                }
                after_cr = byte == b'\r';
            }
        }
        let dominant = if crlf > lf && crlf >= cr {
            LineEnding::Crlf
        } else if cr > lf && cr > crlf {
            LineEnding::Cr
        } else {
            LineEnding::Lf
        };
        LineEndings {
            dominant,
            mixed: [lf, crlf, cr].iter().filter(|&&n| n > 0).count() > 1,
        }
    }
}

/// How many changes a document's text has had: every edit raises it by one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(u64);

impl Revision {
    /// The revision of a document no edit changed yet.
    pub const FIRST: Revision = Revision(0);

    /// The revision `number` changes in, as a language server or a recovery copy names it.
    pub const fn new(number: u64) -> Revision {
        Revision(number)
    }

    /// The number of changes.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The revision after one more edit.
    #[must_use]
    pub(crate) const fn next(self) -> Revision {
        Revision(self.0 + 1)
    }
}

/// The version of its file the document last read or wrote.
#[derive(Debug, Clone)]
struct OnDisk {
    /// The text the file held.
    base: Text,
    /// The revision whose text is `base`, if one is known.
    revision: Option<Revision>,
    stamp: Stamp,
    fingerprint: Fingerprint,
    /// The file is gone.
    removed: bool,
    /// Tells this version from every other one the app has read or written, in this run or an earlier one.
    generation: u64,
    /// How many bytes at the start and at the end of the text are known to be those of `base`, as edits since leave
    /// them, so that telling the two apart compares only what lies between.
    agrees: (usize, usize),
}

/// The bytes at the start and at the end of `text` known to be those of the file's text: all of them when it is the
/// same text, none when it is not known.
fn agreeing(text: &Text, same: bool) -> (usize, usize) {
    let known = if same { text.len_bytes() } else { 0 };
    (known, known)
}

/// The text of one file, shared by every view that shows it, with what is needed to save it, follow its file and
/// colour it. Line breaks stay as the file has them; `line_endings` says which one a new line should get.
#[derive(Debug, Clone)]
pub struct Document {
    text: Text,
    revision: Revision,
    path: Option<PathBuf>,
    format: FileFormat,
    line_endings: LineEndings,
    read_only: Option<ReadOnly>,
    disk: Option<OnDisk>,
    dirty: bool,
    syntax: Syntax,
    language_chosen: bool,
}

impl Default for Document {
    fn default() -> Document {
        Document::untitled("")
    }
}

/// A copy of a document's text and what belongs with it, taken in constant time for another thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub(crate) text: Text,
    pub(crate) revision: Revision,
    pub(crate) path: Option<PathBuf>,
    pub(crate) format: FileFormat,
    pub(crate) base: Option<(Text, u64)>,
}

impl Snapshot {
    /// The text.
    pub fn text(&self) -> &Text {
        &self.text
    }

    /// The document's revision when the copy was taken.
    pub fn revision(&self) -> Revision {
        self.revision
    }
}

/// What a save does when another program changed the file since the document last read or wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangedOnDisk {
    /// Refuses to save.
    Refuse,
    /// Saves over the other program's change.
    Overwrite,
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// The document has no file yet.
    NoPath,
    /// The document may not be saved.
    ReadOnly(ReadOnly),
    /// Another program changed the file since the document last read or wrote it.
    ChangedOnDisk,
    /// The volume refused.
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::NoPath => formatter.write_str("the document has no file"),
            SaveError::ReadOnly(why) => write!(formatter, "the document is read-only: {why:?}"),
            SaveError::ChangedOnDisk => formatter.write_str("the file changed on disk"),
            SaveError::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::Io(error) => Some(error),
            SaveError::NoPath | SaveError::ReadOnly(_) | SaveError::ChangedOnDisk => None,
        }
    }
}

/// A save under way, with the text it writes, so the writing can happen on another thread while editing goes on.
#[derive(Debug, Clone)]
pub struct SaveJob {
    path: PathBuf,
    text: Text,
    format: FileFormat,
    revision: Revision,
    /// What the file held when the document last looked, unless the save may overwrite whatever is there.
    known: Option<(Stamp, Fingerprint)>,
}

impl SaveJob {
    /// Writes the file: in place of the old one, through symbolic links, with the old one's permissions. Refuses with
    /// `ChangedOnDisk` if another program changed the file since the document last looked.
    pub fn run(self) -> Result<Saved, SaveError> {
        if let Some((stamp, fingerprint)) = self.known {
            let now = disk::stamp(&self.path);
            if now.is_reached() && now != stamp {
                let bytes = std::fs::read(&self.path).map_err(SaveError::Io)?;
                if Fingerprint::of(&bytes) != fingerprint {
                    return Err(SaveError::ChangedOnDisk);
                }
            }
        }
        let bytes = disk::encode(self.format, &self.text.to_string());
        let stamp = disk::write(&self.path, &bytes).map_err(SaveError::Io)?;
        Ok(Saved {
            path: self.path,
            text: self.text,
            revision: self.revision,
            stamp,
            fingerprint: Fingerprint::of(&bytes),
        })
    }
}

/// A save that wrote its file, for `Document::end_save`.
#[derive(Debug, Clone)]
pub struct Saved {
    path: PathBuf,
    text: Text,
    revision: Revision,
    stamp: Stamp,
    fingerprint: Fingerprint,
}

/// What the document knows of its file, to look at the file from another thread.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskProbe {
    path: PathBuf,
    stamp: Stamp,
    fingerprint: Fingerprint,
}

impl DiskProbe {
    /// Looks at the file: the date and size decide whether to read it, the content whether it changed.
    pub fn check(&self) -> io::Result<DiskCheck> {
        let now = disk::stamp(&self.path);
        if now == self.stamp {
            return Ok(DiskCheck::Unchanged);
        }
        let content = match FileContent::read(&self.path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DiskCheck::Missing),
            Err(error) => return Err(error),
        };
        if content.fingerprint == self.fingerprint {
            Ok(DiskCheck::Touched(content.stamp))
        } else {
            Ok(DiskCheck::Changed(content))
        }
    }
}

/// What a look at the file found.
#[derive(Debug, Clone)]
pub enum DiskCheck {
    /// The file has the date and size it had.
    Unchanged,
    /// The file holds what it held, under a new date.
    Touched(Stamp),
    /// The file holds something else.
    Changed(FileContent),
    /// There is no file at the path.
    Missing,
}

/// What a document did about its file after `Document::take_disk`.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// Nothing changed.
    Unchanged,
    /// The document had no changes of its own and took the file's text with these edits.
    Reloaded(Vec<Change>),
    /// The file holds the document's text or the text it started from; the document kept its text.
    Kept,
    /// The document took the file's changes into its own with these edits, and is unsaved.
    Merged(Vec<Change>),
    /// Both changed the same places; `Document::adopt_disk` takes `disk` with the text the user settles on.
    Conflict {
        /// The places where both changed.
        conflict: Conflict,
        /// The file's new version.
        disk: FileContent,
    },
    /// The file is gone; the document keeps its text, unsaved.
    Removed,
}

impl Document {
    /// A document without a file, holding `text`.
    pub fn untitled(text: &str) -> Document {
        let text = Text::new(text);
        Document::build(text, None, FileFormat::default(), None, None)
    }

    /// The document of the file at `path`.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Document> {
        let path = path.as_ref();
        Ok(Document::from_file(path.to_path_buf(), FileContent::read(path)?))
    }

    /// The document of the file at `path`, which was read as `content`, say on another thread.
    pub fn from_file(path: PathBuf, content: FileContent) -> Document {
        let text = Text::new(&content.text);
        let disk = OnDisk {
            base: text.clone(),
            revision: Some(Revision::FIRST),
            stamp: content.stamp,
            fingerprint: content.fingerprint,
            removed: false,
            generation: fresh_generation(),
            agrees: agreeing(&text, true),
        };
        Document::build(text, Some(path), content.format, content.read_only, Some(disk))
    }

    /// A document brought back after a crash: its text, its file if it had one, and the text that file held then.
    pub(crate) fn recovered(text: &str, path: Option<PathBuf>, format: FileFormat, base: Option<&str>) -> Document {
        let disk = path.as_ref().map(|_| {
            let base = base.unwrap_or(text);
            OnDisk {
                base: Text::new(base),
                revision: None,
                stamp: Stamp::UNREACHABLE,
                fingerprint: Fingerprint::of(&disk::encode(format, base)),
                removed: false,
                generation: fresh_generation(),
                agrees: (0, 0),
            }
        });
        let mut document = Document::build(Text::new(text), path, format, None, disk);
        document.dirty = true;
        document
    }

    fn build(
        text: Text,
        path: Option<PathBuf>,
        format: FileFormat,
        read_only: Option<ReadOnly>,
        disk: Option<OnDisk>,
    ) -> Document {
        let language = detect(path.as_deref(), &text);
        let mut document = Document {
            line_endings: LineEndings::of(&text),
            syntax: Syntax::new(language, text.line_count()),
            text,
            revision: Revision::FIRST,
            path,
            format,
            read_only,
            disk,
            dirty: false,
            language_chosen: false,
        };
        document.refresh_dirty();
        document
    }

    /// The text.
    pub fn text(&self) -> &Text {
        &self.text
    }

    /// Counts the changes of the text; every edit raises it by one.
    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// Whether the text differs from what its file holds, or, without a file, whether there is any text.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The file, if the document has one.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file now lives at `path`, moved or renamed; nothing is written.
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.redetect_language();
    }

    /// Encoding and byte order mark, as the file had them and a save writes them.
    pub fn format(&self) -> FileFormat {
        self.format
    }

    /// The line breaks of the text as read.
    pub fn line_endings(&self) -> LineEndings {
        self.line_endings
    }

    /// Why the document may not be edited, if it may not.
    pub fn read_only(&self) -> Option<ReadOnly> {
        self.read_only
    }

    /// Makes the document read-only at the user's request, or editable again. A document whose text differs from
    /// its file's bytes stays read-only, and the reason is the error.
    #[cfg(test)]
    pub(crate) fn set_read_only(&mut self, read_only: bool) -> Result<(), ReadOnly> {
        match self.read_only {
            Some(why) if why.is_lossy() => Err(why),
            _ => {
                self.read_only = read_only.then_some(ReadOnly::Requested);
                Ok(())
            }
        }
    }

    /// The language the document is coloured as.
    pub fn language(&self) -> Option<Language> {
        self.syntax.language()
    }

    /// Colours the document as `language` from now on, whatever its file is called.
    pub fn set_language(&mut self, language: Option<Language>) {
        self.language_chosen = true;
        self.syntax = Syntax::new(language, self.text.line_count());
    }

    fn redetect_language(&mut self) {
        let language = detect(self.path.as_deref(), &self.text);
        if !self.language_chosen && language != self.syntax.language() {
            self.syntax = Syntax::new(language, self.text.line_count());
        }
    }

    /// A copy of the text and what belongs with it, for another thread.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            revision: self.revision,
            path: self.path.clone(),
            format: self.format,
            base: self.disk.as_ref().map(|disk| (disk.base.clone(), disk.generation)),
        }
    }

    /// Replaces a range of the text as `edit` says.
    ///
    /// ```
    /// use term_document::{Document, Edit};
    ///
    /// let mut document = Document::untitled("ä = 1");
    /// let change = document.apply(Edit { range_utf16: 4..5, text: "42".into() }).unwrap();
    /// assert_eq!(document.text().to_string(), "ä = 42");
    /// assert_eq!((change.utf16, change.bytes), (4..5, 5..6));
    /// assert_eq!(change.revision, document.revision());
    /// ```
    pub fn apply(&mut self, edit: Edit) -> Result<Change, EditError> {
        if let Some(why) = self.read_only {
            return Err(EditError::ReadOnly(why));
        }
        let start = self.text.utf16_to_byte(edit.range_utf16.start)?;
        let end = self.text.utf16_to_byte(edit.range_utf16.end)?;
        if start > end {
            return Err(EditError::Position(PositionError::OutOfBounds));
        }
        let change = self.replace(ByteEdit {
            range: start..end,
            text: edit.text,
        })?;
        self.refresh_dirty();
        Ok(change)
    }

    /// Makes the text `text` with the fewest edits a comparison by lines finds, so that what stays keeps its place
    /// and an undo step stays small. The edits run from the end of the text to its start, so each one's ranges hold
    /// both in the text before it and in the text before all of them.
    pub fn replace_all(&mut self, text: &str) -> Result<Vec<Change>, EditError> {
        if let Some(why) = self.read_only {
            return Err(EditError::ReadOnly(why));
        }
        Ok(self.replace_text(text)?)
    }

    fn replace_text(&mut self, text: &str) -> Result<Vec<Change>, PositionError> {
        let old = self.text.to_string();
        let (mut changes, mut lines, mut failed) = (Vec::new(), Vec::new(), None);
        for edit in edits_between(&old, text) {
            match self.change_text(edit) {
                Ok((change, edited)) => {
                    changes.push(change);
                    lines.push(edited);
                }
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        // The lines follow once for all edits, of which a new text from an agent or the file may have thousands.
        self.syntax.edited(&lines, &changes);
        if let Some(error) = failed {
            return Err(error);
        }
        debug_assert_eq!(self.syntax.line_count(), self.text.line_count());
        self.refresh_dirty();
        Ok(changes)
    }

    /// Makes one edit, widened where it would end between the CR and the LF of a line break, which a language server
    /// cannot name.
    fn replace(&mut self, edit: ByteEdit) -> Result<Change, PositionError> {
        let (change, lines) = self.change_text(edit)?;
        self.syntax.edited(&[lines], std::slice::from_ref(&change));
        debug_assert_eq!(self.syntax.line_count(), self.text.line_count());
        Ok(change)
    }

    /// The text of `replace` without the lines of the colours, bracket pairs and folds, which follow the lines it
    /// returns.
    fn change_text(&mut self, edit: ByteEdit) -> Result<(Change, LineEdit), PositionError> {
        let ByteEdit { mut range, mut text } = edit;
        if self.text.inside_crlf(range.start) {
            range.start -= 1;
            text.insert(0, '\r');
        }
        if self.text.inside_crlf(range.end) {
            range.end += 1;
            text.push('\n');
        }
        let start = self.text.point(range.start)?;
        let old_end = self.text.point(range.end)?;
        let utf16 = self.text.byte_to_utf16(range.start)?..self.text.byte_to_utf16(range.end)?;
        let old_lines = self.text.line_count();
        if let Some(disk) = &mut self.disk {
            let after = self.text.len_bytes() - range.end;
            disk.agrees = (disk.agrees.0.min(range.start), disk.agrees.1.min(after));
        }
        let rope = self.text.rope_mut();
        let first = rope.byte_to_char(range.start);
        rope.remove(first..rope.byte_to_char(range.end));
        rope.insert(first, &text);
        let new_end = self.text.point(range.start + text.len())?;
        let text_utf16 = self.text.byte_to_utf16(range.start + text.len())? - utf16.start;
        self.revision = self.revision.next();
        // A lone CR in front of the edit and an LF behind it become one line break, ending a line earlier.
        let first = start.line.min(self.text.line_of(range.start));
        let removed = old_end.line - first + 1;
        let added = (removed + self.text.line_count()).saturating_sub(old_lines);
        let change = Change {
            revision: self.revision,
            bytes: range,
            utf16,
            start,
            old_end,
            new_end,
            text_utf16,
            text,
        };
        Ok((change, LineEdit { first, removed, added }))
    }

    fn refresh_dirty(&mut self) {
        self.dirty = match &self.disk {
            Some(disk) if disk.removed => true,
            _ => self.has_own_changes(),
        };
    }

    /// Whether the text holds changes the file never had: unlike `is_dirty`, a file gone since does not count.
    pub fn has_own_changes(&self) -> bool {
        match &self.disk {
            None => !self.text.is_empty(),
            Some(disk) => {
                disk.revision != Some(self.revision)
                    && (disk.base.len_bytes() != self.text.len_bytes() || differs(disk, &self.text))
            }
        }
    }

    /// Starts a save to the document's file, doing as `changed` says if another program changed the file since the
    /// document last read or wrote it.
    pub fn begin_save(&self, changed: ChangedOnDisk) -> Result<SaveJob, SaveError> {
        if let Some(why) = self.read_only {
            return Err(SaveError::ReadOnly(why));
        }
        let path = self.path.clone().ok_or(SaveError::NoPath)?;
        let known = self
            .disk
            .as_ref()
            .filter(|disk| changed == ChangedOnDisk::Refuse && !disk.removed)
            .map(|disk| (disk.stamp, disk.fingerprint));
        Ok(self.job(path, known))
    }

    /// Starts a save to `path`, which becomes the document's file once the save ends; whatever is there is replaced.
    pub fn begin_save_as(&self, path: PathBuf) -> Result<SaveJob, SaveError> {
        match self.read_only {
            Some(why) if why.is_lossy() => Err(SaveError::ReadOnly(why)),
            _ => Ok(self.job(path, None)),
        }
    }

    fn job(&self, path: PathBuf, known: Option<(Stamp, Fingerprint)>) -> SaveJob {
        SaveJob {
            path,
            text: self.text.clone(),
            format: self.format,
            revision: self.revision,
            known,
        }
    }

    /// Takes note of a save that wrote its file; edits made while it wrote stay unsaved. A save of an older revision
    /// than the one known to be in the file, which finished after a newer save took note, changes nothing.
    pub fn end_save(&mut self, saved: Saved) {
        let newer = self.disk.as_ref().and_then(|disk| disk.revision);
        if self.path.as_ref() == Some(&saved.path) && newer.is_some_and(|newer| newer > saved.revision) {
            return;
        }
        if self.path.as_ref() != Some(&saved.path) {
            self.path = Some(saved.path);
            if self.read_only == Some(ReadOnly::Protected) {
                self.read_only = None;
            }
            self.redetect_language();
        }
        self.disk = Some(OnDisk {
            base: saved.text,
            revision: Some(saved.revision),
            stamp: saved.stamp,
            fingerprint: saved.fingerprint,
            removed: false,
            generation: fresh_generation(),
            agrees: agreeing(&self.text, saved.revision == self.revision),
        });
        self.refresh_dirty();
    }

    /// Saves to the document's file and waits for it; see `begin_save`.
    pub fn save(&mut self) -> Result<(), SaveError> {
        let saved = self.begin_save(ChangedOnDisk::Refuse)?.run()?;
        self.end_save(saved);
        Ok(())
    }

    /// Saves to `path`, which becomes the document's file, and waits for it.
    #[cfg(test)]
    pub(crate) fn save_as(&mut self, path: PathBuf) -> Result<(), SaveError> {
        let saved = self.begin_save_as(path)?.run()?;
        self.end_save(saved);
        Ok(())
    }

    /// What to look at the file with, for a document that has one.
    pub fn disk_probe(&self) -> Option<DiskProbe> {
        let path = self.path.clone()?;
        let (stamp, fingerprint) = self
            .disk
            .as_ref()
            .filter(|disk| !disk.removed)
            .map_or((Stamp::UNREACHABLE, Fingerprint::default()), |disk| {
                (disk.stamp, disk.fingerprint)
            });
        Some(DiskProbe {
            path,
            stamp,
            fingerprint,
        })
    }

    /// Follows what a look at the file found. A document without changes of its own takes a changed file as it is;
    /// one with changes keeps them and merges the file's changes in, or reports a conflict and stays as it is.
    pub fn take_disk(&mut self, check: DiskCheck) -> Result<Outcome, EditError> {
        let content = match check {
            DiskCheck::Unchanged => return Ok(Outcome::Unchanged),
            DiskCheck::Touched(stamp) => {
                if let Some(disk) = &mut self.disk {
                    disk.stamp = stamp;
                }
                return Ok(Outcome::Unchanged);
            }
            DiskCheck::Missing => {
                if let Some(disk) = &mut self.disk {
                    disk.removed = true;
                    disk.stamp = Stamp::UNREACHABLE;
                }
                self.refresh_dirty();
                return Ok(Outcome::Removed);
            }
            DiskCheck::Changed(content) => content,
        };
        let buffer = self.text.to_string();
        Ok(match self.reconcile(&buffer, &content) {
            Reconciliation::Reload => Outcome::Reloaded(self.adopt(content, None)?),
            Reconciliation::KeepBuffer => {
                self.adopt(content, Some(&buffer))?;
                Outcome::Kept
            }
            Reconciliation::Merged(text) => Outcome::Merged(self.adopt(content, Some(&text))?),
            Reconciliation::Conflict(conflict) => Outcome::Conflict {
                conflict,
                disk: content,
            },
        })
    }

    /// What `take_disk` would make of the file's new version `disk`, without making it.
    pub fn reconciliation(&self, disk: &FileContent) -> Reconciliation {
        self.reconcile(&self.text.to_string(), disk)
    }

    fn reconcile(&self, buffer: &str, content: &FileContent) -> Reconciliation {
        let lossy = self.read_only.is_some_and(ReadOnly::is_lossy);
        match &self.disk {
            _ if lossy => Reconciliation::Reload,
            _ if content.read_only.is_some_and(ReadOnly::is_lossy) && self.dirty => {
                let base = self
                    .disk
                    .as_ref()
                    .map_or_else(|| buffer.to_string(), |disk| disk.base.to_string());
                Reconciliation::Conflict(merge::whole_text_conflict(&base, buffer, &content.text))
            }
            _ if content.read_only.is_some_and(ReadOnly::is_lossy) => Reconciliation::Reload,
            Some(disk) => merge::reconcile(&disk.base.to_string(), buffer, &content.text),
            None => merge::reconcile(&content.text, buffer, &content.text),
        }
    }

    /// Takes `disk` as the file's current version and makes the text `text`, as the user settled a conflict; the
    /// document is unsaved unless `text` is what the file holds.
    pub fn adopt_disk(&mut self, disk: FileContent, text: &str) -> Result<Vec<Change>, EditError> {
        self.adopt(disk, Some(text))
    }

    /// Takes `disk` as the file's current version with the text `text`, or the file's text.
    fn adopt(&mut self, disk: FileContent, text: Option<&str>) -> Result<Vec<Change>, EditError> {
        let same = text.is_none_or(|text| text == disk.text);
        let changes = self.replace_text(text.unwrap_or(&disk.text))?;
        let base = if same { self.text.clone() } else { Text::new(&disk.text) };
        self.disk = Some(OnDisk {
            base,
            revision: same.then_some(self.revision),
            stamp: disk.stamp,
            fingerprint: disk.fingerprint,
            removed: false,
            generation: fresh_generation(),
            agrees: agreeing(&self.text, same),
        });
        self.format = disk.format;
        self.read_only = match (disk.read_only, self.read_only) {
            (Some(why), _) if why.is_lossy() => Some(why),
            (_, Some(ReadOnly::Requested)) => Some(ReadOnly::Requested),
            (Some(ReadOnly::Protected), _) => Some(ReadOnly::Protected),
            _ => None,
        };
        self.line_endings = LineEndings::of(&self.text);
        self.refresh_dirty();
        Ok(changes)
    }

    /// Lexes what the lines `lines` need, visible ones first, and returns the lines whose colours changed.
    #[cfg(test)]
    pub(crate) fn highlight(&mut self, lines: Range<usize>) -> Option<Range<usize>> {
        self.syntax.highlight(&self.text, lines, None)
    }

    /// `highlight`, catching up on the lines above `lines` for at most `budget`; the lines of `lines` it does not reach
    /// are lexed from a guessed state, which `highlight_more` checks once it gets there.
    pub fn highlight_within(&mut self, lines: Range<usize>, budget: Duration) -> Option<Range<usize>> {
        self.syntax.highlight(&self.text, lines, Some(Instant::now() + budget))
    }

    /// Lexes on through the rest of the document for at most `budget`, and returns the lines whose colours changed.
    pub fn highlight_more(&mut self, budget: Duration) -> Option<Range<usize>> {
        self.syntax.highlight_until(&self.text, Instant::now() + budget)
    }

    /// Whether every line is lexed from its true state.
    pub fn is_highlighted(&self) -> bool {
        self.syntax.is_complete()
    }

    /// The coloured stretches of line `line`, as far as it was lexed.
    pub fn spans(&self, line: usize) -> &[Span] {
        self.syntax.spans(line)
    }

    /// The styled stretches of line `line` of a Markdown document, as far as they are known; none in other languages.
    pub fn styles(&self, line: usize) -> &[Styled] {
        self.syntax.styles(line)
    }

    /// The bracket at UTF-16 offset `offset` or right before it, outside strings and comments, with its partner.
    #[cfg(test)]
    pub(crate) fn matching_bracket(&mut self, offset: usize) -> Option<BracketPair> {
        self.syntax.matching_bracket(&self.text, offset)
    }

    /// The folds by indentation, with tabs reaching the next multiple of `tab_width`.
    pub fn folds(&mut self, tab_width: usize) -> Vec<Fold> {
        self.syntax.folds(&self.text, tab_width)
    }
}

/// Whether `text`, as long as the file's text of `disk`, differs from it between the ends known to agree.
fn differs(disk: &OnDisk, text: &Text) -> bool {
    let length = text.len_bytes();
    let (start, end) = disk.agrees;
    if start.saturating_add(end) >= length {
        return false;
    }
    match text.same_bytes(&disk.base, start..length - end) {
        Some(same) => !same,
        None => disk.base != *text,
    }
}

/// A number no earlier version got: the time in nanoseconds, or one more than the last if the clock has not moved.
fn fresh_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| u64::try_from(time.as_nanos()).unwrap_or(u64::MAX));
    let mut last = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

fn detect(path: Option<&Path>, text: &Text) -> Option<Language> {
    let first_line: String = text
        .rope()
        .chars()
        .take(256)
        .take_while(|&c| c != '\n' && c != '\r')
        .collect();
    Language::detect(path, &first_line)
}

#[cfg(test)]
mod tests;
