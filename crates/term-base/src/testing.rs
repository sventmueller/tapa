//! What the crates' tests and the app's self-tests share, behind the feature `testing`: a folder of its own for each
//! test that goes away with it, a writer that fails, and a thread's CPU time for timing budgets.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use crate::sync::LockIgnoringPoison;

/// An empty folder of its own for one test, removed when it is dropped. Its path has no symbolic links, so it
/// compares equal to what the system reports for files in it (`/private/var` for `/var`).
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// A new empty folder whose name starts with `name`; no other test of this process gets the same one.
    pub fn new(name: &str) -> Scratch {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("term-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        Scratch(dir.canonicalize().unwrap_or(dir))
    }

    /// The folder's path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The folder's path, leaving the folder in place, for what must outlive the test's own scope.
    pub fn keep(self) -> PathBuf {
        let path = self.0.clone();
        std::mem::forget(self);
        path
    }
}

/// A new empty folder for one test; see [`Scratch`].
pub fn scratch(name: &str) -> Scratch {
    Scratch::new(name)
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl PartialEq<Scratch> for Path {
    fn eq(&self, scratch: &Scratch) -> bool {
        self == scratch.0
    }
}

impl PartialEq<Scratch> for &Path {
    fn eq(&self, scratch: &Scratch) -> bool {
        *self == scratch.0
    }
}

impl PartialEq<Scratch> for PathBuf {
    fn eq(&self, scratch: &Scratch) -> bool {
        *self == scratch.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A writer that takes a number of bytes and then fails every write, as a full disk or a pipe whose reader died does.
/// Its clones share what it took.
#[derive(Debug, Clone)]
pub struct FailingWriter {
    limit: usize,
    kind: io::ErrorKind,
    taken: Arc<Mutex<Vec<u8>>>,
}

impl FailingWriter {
    /// A writer that takes `limit` bytes and fails with `kind` once they are taken.
    pub fn after(limit: usize, kind: io::ErrorKind) -> FailingWriter {
        FailingWriter {
            limit,
            kind,
            taken: Arc::default(),
        }
    }

    /// What the writer and its clones took.
    pub fn taken(&self) -> Vec<u8> {
        self.taken.lock_ignoring_poison().clone()
    }
}

impl io::Write for FailingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut taken = self.taken.lock_ignoring_poison();
        let room = self.limit.saturating_sub(taken.len());
        if room == 0 && !buf.is_empty() {
            return Err(io::Error::from(self.kind));
        }
        let n = buf.len().min(room);
        taken.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The CPU time of the calling thread, so other processes competing for the cores do not count against a budget.
pub fn thread_time() -> std::time::Duration {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    std::time::Duration::new(
        u64::try_from(now.tv_sec).unwrap_or(0),
        u32::try_from(now.tv_nsec).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_folder_is_its_own_and_goes_away_with_it() {
        let one = scratch("testing");
        let two = scratch("testing");
        assert_ne!(one.path(), two.path());
        std::fs::write(one.join("file"), "x").unwrap();
        let kept = one.to_path_buf();
        drop(one);
        assert!(!kept.exists());
        assert!(two.is_dir());
    }

    #[test]
    fn a_failing_writer_takes_its_bytes_and_then_refuses() {
        use std::io::Write;
        let mut writer = FailingWriter::after(3, io::ErrorKind::StorageFull);
        let seen = writer.clone();
        let error = writer.write_all(b"abcd").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(seen.taken(), b"abc");
        assert_eq!(writer.write(b"").unwrap(), 0);
        assert!(writer.write(b"e").is_err());
    }

    #[test]
    fn the_thread_time_grows_with_work_on_the_thread() {
        let start = thread_time();
        let mut spun = 0_u64;
        while thread_time() == start {
            spun = std::hint::black_box(spun.wrapping_add(1));
        }
        assert!(thread_time() > start);
    }
}
