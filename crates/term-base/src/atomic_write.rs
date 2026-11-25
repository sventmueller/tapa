//! Replacing a file as a whole, so that after a crash or a power loss it holds the old content or the new one, never
//! a part or an empty file: the settings, the trusted folders, the thread index and the shell scripts are written so.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Writes `bytes` as the file at `path`, creating its folder: into a new file beside it that is flushed to the disk
/// and then takes its place, after which the folder is flushed, so the new name survives a power loss too. Every
/// write has a scratch file of its own, so two writers of one file at once leave one of the two whole.
pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let name = path.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy());
    let (temporary, mut file) = create_beside(dir, &name)?;
    let written = file
        .write_all(bytes)
        .and_then(|()| flush(&file))
        .and_then(|()| fs::rename(&temporary, path));
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    // Some file systems refuse to flush a folder; the file has its new content and name either way.
    let _ = File::open(dir).and_then(|folder| flush(&folder));
    Ok(())
}

/// Hands `file` to the disk with `fsync`; std's `sync_all` is `F_FULLFSYNC` on macOS, which also empties the drive's
/// cache and can hold the main thread, where settings are written, for tens of milliseconds.
fn flush(file: &File) -> io::Result<()> {
    rustix::fs::fsync(file).map_err(io::Error::from)
}

/// Writes `value` as pretty JSON with a final newline, as [`write`] does.
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write(path, &bytes)
}

/// Whether `bytes`, a file's content, are what a write without a flush that a power loss cut off leaves: nothing, or
/// zeros where the data never reached the disk. Such a file holds nothing to keep and may be written over.
pub fn is_torn(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| b.is_ascii_whitespace() || *b == 0)
}

/// A new, empty file beside the one called `name` in `dir`, named after it, the process and a counter.
fn create_beside(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{name}.{}-{n}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::scratch;

    #[test]
    fn a_file_is_written_into_a_new_folder_and_replaced_without_a_scratch_file_left() {
        let dir = scratch("atomic-write");
        let path = dir.join("nested/state.json");
        write_json(&path, &serde_json::json!({"a": 1})).unwrap();
        write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_replacement_keeps_the_old_file_and_leaves_no_scratch_file() {
        let dir = scratch("atomic-write-failed");
        let path = dir.join("state.json");
        fs::create_dir_all(path.join("blocker")).unwrap();
        assert!(write(&path, b"new").is_err());
        assert!(path.join("blocker").is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn two_writers_of_one_file_at_once_always_leave_one_of_them_whole() {
        let dir = scratch("atomic-write-race");
        let path = dir.join("index.json");
        let writers: Vec<_> = b"ab"
            .iter()
            .map(|&byte| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let content = vec![byte; 64 * 1024];
                    for _ in 0..40 {
                        write(&path, &content).unwrap();
                    }
                })
            })
            .collect();
        for _ in 0..200 {
            if let Ok(bytes) = fs::read(&path) {
                assert_eq!(bytes.len(), 64 * 1024, "a torn file");
                assert!(bytes.iter().all(|b| *b == bytes[0]), "two writers' bytes in one file");
            }
        }
        for writer in writers {
            writer.join().unwrap();
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
