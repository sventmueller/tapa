//! Live reload (F160): the settings file is looked at twice a second, and a change in its time or size calls back.
//! Polling, because editors replace the file instead of writing into it, which ends a watch on the old file.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// How often the file is looked at.
const PERIOD: Duration = Duration::from_millis(500);

/// Watches a file while it lives.
#[derive(Debug)]
pub struct Watcher {
    stop: Arc<AtomicBool>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Calls `changed` on a thread of its own whenever `path` changes, appears or goes away, until the watcher is dropped.
pub fn watch(path: PathBuf, changed: impl Fn() + Send + 'static) -> std::io::Result<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let theirs = Arc::clone(&stop);
    let mut last = stamp(&path);
    std::thread::Builder::new().name("config-watch".into()).spawn(move || {
        while !theirs.load(Ordering::Relaxed) {
            std::thread::sleep(PERIOD);
            let now = stamp(&path);
            if now != last {
                last = now;
                changed();
            }
        }
    })?;
    Ok(Watcher { stop })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn a_change_to_the_file_is_heard() {
        let dir = term_base::testing::scratch("config-watch");
        let path = dir.join("config.toml");
        let (tx, rx) = channel();
        let _watcher = watch(path.clone(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::fs::write(&path, "version = 1\n").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok());
    }
}
