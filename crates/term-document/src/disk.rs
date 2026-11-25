//! A file as the document reads and writes it: decoding, what makes a file read-only, its identity and its stamp, and
//! a write that leaves either the old file or the new one.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use splat_file_state::{Encoding, FileFormat, Fingerprint, Stamp};

/// The largest file a document opens.
pub(crate) const MAX_FILE_BYTES: u64 = 512 << 20;

/// How much of a file is looked at for a NUL byte, as Git does to tell binary files from text.
const BINARY_PROBE: usize = 8000;

/// Why a document may not be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnly {
    /// The file holds a NUL byte near its start; its text is shown with replacement characters and never saved.
    Binary,
    /// The file is neither UTF-8 nor UTF-16 with a byte order mark; its text is shown with replacement characters and
    /// never saved, since guessing the encoding would corrupt it.
    Undecodable,
    /// The file allows no one to write it.
    Protected,
    /// The user asked for it.
    Requested,
}

impl ReadOnly {
    /// Whether the text shown differs from the file, so that no save may write it.
    pub(crate) fn is_lossy(self) -> bool {
        matches!(self, ReadOnly::Binary | ReadOnly::Undecodable)
    }
}

/// A version of a file as read: its text without the byte order mark, its format, and its stamp and fingerprint to
/// tell later versions from it.
#[derive(Debug, Clone)]
pub struct FileContent {
    /// The text, with its line breaks as the file has them.
    pub text: String,
    /// Encoding and byte order mark.
    pub(crate) format: FileFormat,
    /// Why the document of this file may not be edited, if it may not.
    pub(crate) read_only: Option<ReadOnly>,
    /// Modification time and size when read.
    pub(crate) stamp: Stamp,
    /// The fingerprint of the bytes read.
    pub(crate) fingerprint: Fingerprint,
}

impl FileContent {
    /// Reads the file at `path`, following symbolic links. A file that changes while it is read is read again.
    pub fn read(path: &Path) -> io::Result<FileContent> {
        term_base::main_guard::blocking("reading a file");
        // Opening a named pipe waits until a program writes into it, so only files and folders are opened.
        let kind = fs::metadata(path)?.file_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        let mut file = File::open(path)?;
        let mut tries = 0;
        loop {
            let before = file.metadata()?;
            if before.is_dir() {
                return Err(io::Error::from(io::ErrorKind::IsADirectory));
            }
            if before.len() > MAX_FILE_BYTES {
                return Err(io::Error::from(io::ErrorKind::FileTooLarge));
            }
            let mut bytes = Vec::with_capacity(usize::try_from(before.len()).unwrap_or(0));
            io::Seek::rewind(&mut file)?;
            (&mut file).take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
            if u64::try_from(bytes.len()).is_ok_and(|len| len > MAX_FILE_BYTES) {
                return Err(io::Error::from(io::ErrorKind::FileTooLarge));
            }
            let after = file.metadata()?;
            tries += 1;
            let settled =
                stamp_of(&before) == stamp_of(&after) && u64::try_from(bytes.len()).is_ok_and(|len| len == after.len());
            if settled || tries == 3 {
                return Ok(FileContent::decode(bytes, stamp_of(&after), is_protected(&after)));
            }
        }
    }

    /// The content of a file that holds `bytes`.
    pub(crate) fn decode(bytes: Vec<u8>, stamp: Stamp, protected: bool) -> FileContent {
        let fingerprint = Fingerprint::of(&bytes);
        let wide = bytes.starts_with(b"\xFF\xFE") || bytes.starts_with(b"\xFE\xFF");
        let binary = !wide && bytes[..bytes.len().min(BINARY_PROBE)].contains(&0);
        let kept = (wide || std::str::from_utf8(&bytes).is_err()).then(|| bytes.clone());
        let (format, text, lossy) = match Encoding::read(bytes) {
            Some((encoding, text)) => {
                let (format, body) = FileFormat::of(&text);
                let text = if format.bom { body.to_string() } else { text };
                (FileFormat { encoding, ..format }, text, None)
            }
            None => {
                let bytes = kept.unwrap_or_default();
                let why = if binary {
                    ReadOnly::Binary
                } else {
                    ReadOnly::Undecodable
                };
                (
                    FileFormat::default(),
                    String::from_utf8_lossy(&bytes).into_owned(),
                    Some(why),
                )
            }
        };
        let read_only = lossy
            .or(binary.then_some(ReadOnly::Binary))
            .or(protected.then_some(ReadOnly::Protected));
        FileContent {
            text,
            format,
            read_only,
            stamp,
            fingerprint,
        }
    }
}

/// The bytes a file in `format` holds for `text`. Line breaks are written as the text has them, so a save changes
/// only what was edited and the file afterwards holds exactly the text.
pub(crate) fn encode(format: FileFormat, text: &str) -> Vec<u8> {
    FileFormat { crlf: false, ..format }.encode(text)
}

/// The stamp of the file at `path` now, following symbolic links; unreachable if there is none.
pub(crate) fn stamp(path: &Path) -> Stamp {
    fs::metadata(path).map_or(Stamp::UNREACHABLE, |meta| stamp_of(&meta))
}

fn stamp_of(meta: &Metadata) -> Stamp {
    meta.modified()
        .map_or(Stamp::UNREACHABLE, |modified| Stamp::new(modified, meta.len()))
}

fn is_protected(meta: &Metadata) -> bool {
    meta.permissions().mode() & 0o222 == 0
}

/// What tells two paths to one file apart from two files: on a volume that ignores case, through symbolic or hard
/// links, both paths give the same key.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileKey {
    device: u64,
    inode: u64,
}

#[cfg(test)]
impl FileKey {
    /// The key of the file at `path`, following symbolic links.
    pub(crate) fn of(path: &Path) -> io::Result<FileKey> {
        let meta = fs::metadata(path)?;
        Ok(FileKey {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
}

/// The file a save through `path` writes: the end of its symbolic links, spelled as the volume spells it, so a
/// save neither replaces a link by a file nor renames a file on a volume that ignores case.
pub(crate) fn save_target(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut current = path.to_path_buf();
            for _ in 0..40 {
                match fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        let link = fs::read_link(&current)?;
                        current = current.parent().unwrap_or(Path::new("/")).join(link);
                    }
                    _ => break,
                }
            }
            let name = current
                .file_name()
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            let folder = current
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            Ok(fs::canonicalize(folder)?.join(name))
        }
        Err(error) => Err(error),
    }
}

/// Writes `bytes` as the file at `path` and returns its new stamp. The bytes go to a new file beside it that then
/// takes its place, with the old file's permissions, after which the folder is flushed, so a crash or a power loss
/// leaves the old file or the new one. A file with
/// further hard links or another owner is written in place instead, since replacing it would cut the links or
/// change the owner.
pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<Stamp> {
    let target = save_target(path)?;
    let existing = fs::metadata(&target).ok();
    if existing.as_ref().is_some_and(|meta| meta.nlink() > 1) {
        return write_in_place(&target, bytes);
    }
    let folder = target.parent().unwrap_or(Path::new("/"));
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let (temporary, mut file) = match create_beside(folder, &name) {
        Ok(created) => created,
        Err(error) if existing.is_some() && error.kind() == io::ErrorKind::PermissionDenied => {
            return write_in_place(&target, bytes);
        }
        Err(error) => return Err(error),
    };
    let written = (|| {
        if let Some(meta) = &existing {
            let own = file.metadata()?;
            if own.uid() != meta.uid() {
                return Ok(false);
            }
            if own.gid() != meta.gid() && std::os::unix::fs::fchown(&file, None, Some(meta.gid())).is_err() {
                return Ok(false);
            }
            file.set_permissions(fs::Permissions::from_mode(meta.permissions().mode() & 0o7777))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &target)?;
        Ok(true)
    })();
    match written {
        Ok(true) => {
            // Some file systems refuse to flush a folder; the file has its new content and name either way.
            let _ = File::open(folder).and_then(|folder| folder.sync_all());
            Ok(stamp(&target))
        }
        Ok(false) => {
            let _ = fs::remove_file(&temporary);
            write_in_place(&target, bytes)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

fn write_in_place(target: &Path, bytes: &[u8]) -> io::Result<Stamp> {
    let mut file = OpenOptions::new().write(true).truncate(true).open(target)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(stamp_of(&file.metadata()?))
}

fn create_beside(folder: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = folder.join(format!(".{name}.{}-{n}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    pub(crate) use term_base::testing::scratch;

    fn content(bytes: &[u8]) -> FileContent {
        FileContent::decode(bytes.to_vec(), Stamp::UNREACHABLE, false)
    }

    #[test]
    fn text_keeps_its_line_breaks_and_loses_only_the_byte_order_mark() {
        let read = content("\u{FEFF}a\r\nb\nc\r".as_bytes());
        assert_eq!(read.text, "a\r\nb\nc\r");
        assert!(read.format.bom);
        assert_eq!(read.read_only, None);
        assert_eq!(encode(read.format, &read.text), "\u{FEFF}a\r\nb\nc\r".as_bytes());
    }

    #[test]
    fn a_crlf_file_is_written_back_as_the_text_has_it() {
        let read = content(b"a\r\nb\r\n");
        assert!(read.format.crlf);
        assert_eq!(encode(read.format, "a\r\nb\r\nc\n"), b"a\r\nb\r\nc\n");
    }

    #[test]
    fn utf16_opens_as_text_and_saves_as_utf16() {
        let read = content(b"\xFF\xFEa\0\n\0");
        assert_eq!(read.text, "a\n");
        assert_eq!(read.format.encoding, Encoding::Utf16Le);
        assert_eq!(read.read_only, None);
        assert_eq!(encode(read.format, "b"), b"\xFF\xFEb\0");
    }

    #[test]
    fn binary_and_undecodable_files_open_read_only_with_replacement_characters() {
        let binary = content(b"PK\x03\x04\0\0\xFF");
        assert_eq!(binary.read_only, Some(ReadOnly::Binary));
        assert!(binary.text.contains('\u{FFFD}'));
        let nul_in_utf8 = content(b"text\0more");
        assert_eq!(nul_in_utf8.read_only, Some(ReadOnly::Binary));
        assert_eq!(nul_in_utf8.text, "text\0more");
        let latin1 = content(b"Gr\xFC\xDFe");
        assert_eq!(latin1.read_only, Some(ReadOnly::Undecodable));
        assert_eq!(latin1.text, "Gr\u{FFFD}\u{FFFD}e");
        assert!(ReadOnly::Undecodable.is_lossy() && !ReadOnly::Protected.is_lossy());
    }

    #[test]
    fn a_save_replaces_the_file_and_keeps_its_permissions() {
        let dir = scratch("permissions");
        let path = dir.join("script.sh");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o751)).unwrap();
        let before = FileKey::of(&path).unwrap();
        let stamp = write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o751);
        assert_ne!(
            FileKey::of(&path).unwrap(),
            before,
            "a new file took the old one's place"
        );
        assert_eq!(stamp, super::stamp(&path));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temporary file stays behind");
    }

    #[test]
    fn a_save_into_a_folder_that_cannot_be_flushed_still_replaces_the_file() {
        let dir = scratch("unflushable");
        let folder = dir.join("write-only");
        fs::create_dir(&folder).unwrap();
        let path = folder.join("file.txt");
        fs::write(&path, "old").unwrap();
        let before = FileKey::of(&path).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o300)).unwrap();
        let written = write(&path, b"new");
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        written.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_ne!(
            FileKey::of(&path).unwrap(),
            before,
            "a new file took the old one's place"
        );
        assert_eq!(
            fs::read_dir(&folder).unwrap().count(),
            1,
            "no temporary file stays behind"
        );
    }

    #[test]
    fn a_save_through_a_symbolic_link_writes_its_target_and_keeps_the_link() {
        let dir = scratch("symlink");
        fs::create_dir(dir.join("real")).unwrap();
        let target = dir.join("real/file.txt");
        fs::write(&target, "old").unwrap();
        let link = dir.join("link.txt");
        symlink("real/file.txt", &link).unwrap();
        write(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(FileKey::of(&link).unwrap(), FileKey::of(&target).unwrap());

        let dangling = dir.join("dangling.txt");
        symlink("real/new.txt", &dangling).unwrap();
        write(&dangling, b"born").unwrap();
        assert_eq!(fs::read(dir.join("real/new.txt")).unwrap(), b"born");
        assert!(fs::symlink_metadata(&dangling).unwrap().file_type().is_symlink());
    }

    #[test]
    fn a_file_with_another_hard_link_is_written_in_place() {
        let dir = scratch("hardlink");
        let path = dir.join("a.txt");
        fs::write(&path, "old").unwrap();
        fs::hard_link(&path, dir.join("b.txt")).unwrap();
        write(&path, b"new").unwrap();
        assert_eq!(fs::read(dir.join("b.txt")).unwrap(), b"new");
        assert_eq!(FileKey::of(&path).unwrap(), FileKey::of(&dir.join("b.txt")).unwrap());
    }

    #[test]
    fn a_save_keeps_the_spelling_of_a_name_on_a_volume_that_ignores_case() {
        let dir = scratch("case");
        let path = dir.join("README.md");
        fs::write(&path, "old").unwrap();
        let other_case = dir.join("readme.md");
        if !other_case.exists() {
            return;
        }
        assert_eq!(FileKey::of(&other_case).unwrap(), FileKey::of(&path).unwrap());
        write(&other_case, b"new").unwrap();
        let names: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["README.md"]);
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn reading_tells_a_protected_file_and_refuses_a_folder() {
        let dir = scratch("protected");
        let path = dir.join("locked.txt");
        fs::write(&path, "text").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        let read = FileContent::read(&path).unwrap();
        assert_eq!(read.read_only, Some(ReadOnly::Protected));
        assert_eq!(read.stamp, stamp(&path));
        assert_eq!(read.fingerprint, Fingerprint::of(b"text"));
        assert_eq!(FileContent::read(&dir).unwrap_err().kind(), io::ErrorKind::IsADirectory);
        assert_eq!(stamp(&dir.join("missing")), Stamp::UNREACHABLE);
    }

    #[test]
    fn reading_refuses_what_is_neither_a_file_nor_a_folder() {
        let device = Path::new("/dev/zero");
        assert_eq!(
            FileContent::read(device).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        let dir = scratch("special");
        let link = dir.join("zero");
        symlink(device, &link).unwrap();
        assert_eq!(FileContent::read(&link).unwrap_err().kind(), io::ErrorKind::Unsupported);
        assert_eq!(
            FileContent::read(&dir.join("missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
