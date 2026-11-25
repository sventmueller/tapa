//! Which hook made a git command fail. git exits with 1 whatever refused, so its trace2 event stream, which records
//! every hook it starts and how each ended, tells.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::run::{Call, Git, Output};

/// Runs `call` with a trace of its hooks; the second value names the first hook that failed.
pub(crate) fn run(git: &Git, call: Call<'_>) -> Result<(Output, Option<String>)> {
    let dir = TraceDir::new()?;
    let trace = dir.path().join("trace.json");
    let output = git.run(call.env("GIT_TRACE2_EVENT", &trace))?;
    let failed = std::fs::read_to_string(&trace).ok().and_then(|text| failed_hook(&text));
    Ok((output, failed))
}

/// A folder of one call's own for git's trace, which only the user may enter, removed when dropped.
struct TraceDir(PathBuf);

impl TraceDir {
    fn new() -> io::Result<TraceDir> {
        let parent = std::env::temp_dir();
        for _ in 0..16 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            let path = parent.join(format!("term-git-trace-{:016x}", hasher.finish()));
            // A name someone else took, a link included, fails here and is never used.
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(TraceDir(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TraceDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The first hook of the top git process that exited with anything but 0.
fn failed_hook(trace: &str) -> Option<String> {
    let mut started: HashMap<i64, String> = HashMap::new();
    for line in trace.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // git processes started by hooks write to the same file; their session ids continue the parent's after a slash.
        if event["sid"].as_str().is_none_or(|sid| sid.contains('/')) {
            continue;
        }
        let Some(child) = event["child_id"].as_i64() else {
            continue;
        };
        match event["event"].as_str() {
            Some("child_start") if event["child_class"] == "hook" => {
                started.insert(child, event["hook_name"].as_str().unwrap_or("hook").to_string());
            }
            Some("child_exit") if event["code"].as_i64() != Some(0) => {
                if let Some(hook) = started.remove(&child) {
                    return Some(hook);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_failing_hook_of_the_top_process_is_named() {
        let trace = r#"{"event":"version","sid":"A"}
{"event":"child_start","sid":"A","child_id":0,"child_class":"hook","hook_name":"pre-commit"}
{"event":"child_start","sid":"A/B","child_id":0,"child_class":"hook","hook_name":"nested"}
{"event":"child_exit","sid":"A/B","child_id":0,"code":1}
{"event":"child_exit","sid":"A","child_id":0,"code":0}
{"event":"child_start","sid":"A","child_id":1,"child_class":"hook","hook_name":"commit-msg"}
{"event":"child_exit","sid":"A","child_id":1,"code":3}
not json"#;
        assert_eq!(failed_hook(trace).as_deref(), Some("commit-msg"));
    }

    #[test]
    fn hooks_that_succeed_and_other_children_are_no_failure() {
        let trace = r#"{"event":"child_start","sid":"A","child_id":0,"child_class":"?"}
{"event":"child_exit","sid":"A","child_id":0,"code":1}
{"event":"child_start","sid":"A","child_id":1,"child_class":"hook","hook_name":"pre-commit"}
{"event":"child_exit","sid":"A","child_id":1,"code":0}"#;
        assert_eq!(failed_hook(trace), None);
    }
}
