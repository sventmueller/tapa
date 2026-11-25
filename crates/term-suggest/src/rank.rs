//! Ranking: how often and how lately a command ran, weighed up in the same folder, branch and after the same command.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::history::Entry;

/// Where the user is typing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Context {
    /// The session's folder.
    pub folder: PathBuf,
    /// The folder's branch.
    pub branch: Option<String>,
    /// The command that ran before in this session.
    pub previous: Option<String>,
    /// How that command ended.
    pub exit_code: Option<i32>,
}

/// Half the weight of a run is gone after this many seconds.
const HALF_LIFE: f64 = 7.0 * 24.0 * 3600.0;

/// The command among `entries` that `accept` lets through with the highest score in `context`.
pub(crate) fn best(entries: &[Entry], context: &Context, now: u64, accept: impl Fn(&str) -> bool) -> Option<String> {
    let mut scores: HashMap<&str, f64> = HashMap::new();
    for entry in entries.iter().rev().take(20_000) {
        if !accept(&entry.line) {
            continue;
        }
        let age = now.saturating_sub(entry.started) as f64;
        let mut weight = 0.5f64.powf(age / HALF_LIFE);
        if entry.folder == context.folder {
            weight *= 3.0;
        }
        if entry.branch.is_some() && entry.branch == context.branch {
            weight *= 1.5;
        }
        if entry.previous.is_some() && entry.previous == context.previous {
            weight *= 2.0;
        }
        if entry.exit_code.is_some_and(|code| code != 0) {
            weight *= 0.3;
        }
        *scores.entry(&entry.line).or_insert(0.0) += weight;
    }
    scores
        .into_iter()
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.len().cmp(&a.0.len())))
        .map(|(line, _)| line.to_string())
}
