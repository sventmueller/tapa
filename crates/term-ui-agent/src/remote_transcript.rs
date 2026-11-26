//! The conversation of a Claude session in a remote workspace (E25), whose transcript the CLI keeps on the machine
//! there: copied into a private file of this Mac while the timeline is rebuilt from it, then removed.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use term_thread::Thread;

/// How long a tab that opens again waits for its workspace's connection, which may still ask for a password.
pub(crate) const WAIT_FOR_CONNECTION: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(1);

/// Whether a session in `cwd` runs on another machine.
pub(crate) fn is_remote(cwd: &Path) -> bool {
    term_remote::registry::bin_for(cwd).is_some()
}

/// `thread` with the conversation of Claude's session `session` in the remote workspace folder `cwd`, waiting up to
/// `patience` for the workspace's connection; `None` where the machine has no transcript or the connection stayed
/// closed.
pub(crate) fn rebuild(thread: Thread, cwd: &Path, session: &str, patience: Duration) -> Option<Thread> {
    let started = Instant::now();
    let transcript = loop {
        match term_remote::claude_transcript(cwd, session) {
            Ok(found) => break found?,
            Err(_) if started.elapsed() < patience => std::thread::sleep(RETRY),
            Err(error) => {
                crate::common::diag::report("reading a remote session's transcript", error);
                return None;
            }
        }
    };
    let folder = term_base::cache_dir().join("remote-transcripts");
    let file = folder.join(format!("{session}.jsonl"));
    let written = std::fs::create_dir_all(&folder).and_then(|()| {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&file)?
            .write_all(&transcript)
    });
    let rebuilt = written.and_then(|()| term_claude::history::rebuild(thread, &file));
    let _ = std::fs::remove_file(&file);
    rebuilt.ok()
}
