//! Reading a file the user is about to open on the disk lane, so that opening it finds it in the system's page cache.

use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// How many reads may run or wait at once; the disk lane also saves the user's files.
const IN_FLIGHT: usize = 2;

/// The largest file read ahead.
const LIMIT: u64 = 1 << 20;

static READ_AHEAD: term_base::lane::Lane =
    term_base::lane::Lane::with_start("term-file-prewarm", 1, 2, term_base::priority::background);

/// The reads ahead of the folder tree.
#[derive(Default)]
pub(crate) struct Prewarm {
    in_flight: Arc<AtomicUsize>,
    last: RefCell<Option<PathBuf>>,
}

/// Counts a read as done however it ends.
struct Done(Arc<AtomicUsize>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Prewarm {
    /// Reads `path` ahead on the disk lane, unless it was the last path asked for or enough reads run already.
    pub(crate) fn file(&self, path: &Path) {
        if !self.admit(path) {
            return;
        }
        let done = Done(self.in_flight.clone());
        let path = path.to_path_buf();
        // A refused job drops `done` with it.
        let _ = READ_AHEAD.try_run(move || {
            let _done = done;
            let _ = read(&path);
        });
    }

    /// Whether a read of `path` may start now, counting it as running if so.
    fn admit(&self, path: &Path) -> bool {
        if self.last.borrow().as_deref() == Some(path) || self.in_flight.load(Ordering::Relaxed) >= IN_FLIGHT {
            return false;
        }
        *self.last.borrow_mut() = Some(path.to_path_buf());
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// Reads the regular file `path` to its end if it is at most `LIMIT` bytes long.
fn read(path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > LIMIT {
        return Ok(());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(LIMIT).read_to_end(&mut bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_path_is_read_once_in_a_row_and_another_after_it() {
        let warm = Prewarm::default();
        assert!(warm.admit(Path::new("/a")));
        assert!(!warm.admit(Path::new("/a")));
        assert!(warm.admit(Path::new("/b")));
        assert!(!warm.admit(Path::new("/c")), "two reads run already");
        warm.in_flight.store(0, Ordering::Relaxed);
        assert!(warm.admit(Path::new("/a")));
    }

    #[test]
    fn a_read_ends_its_count_and_skips_folders_and_large_files() {
        let scratch = term_base::testing::Scratch::new("prewarm");
        let dir = scratch.path().to_path_buf();
        let small = dir.join("small.txt");
        std::fs::write(&small, b"hello").unwrap();
        assert!(read(&small).is_ok());
        assert!(read(&dir).is_ok());
        assert!(read(&dir.join("missing")).is_err());
        let warm = Prewarm::default();
        warm.file(&small);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while warm.in_flight.load(Ordering::Relaxed) > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(warm.in_flight.load(Ordering::Relaxed), 0);
    }
}
