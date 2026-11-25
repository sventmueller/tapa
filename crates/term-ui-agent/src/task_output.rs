//! The output of a background task as it grows: where to read it at a given moment and what a read adds to what is
//! shown.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

use term_thread::{BackgroundItem, EntryKind, Thread, ToolView};

/// The most of a file a tab shows: the end of a longer one, which the editor opens whole.
pub(crate) const MAX_SHOWN: u64 = 2 * 1024 * 1024;

/// What a read changes in the shown output.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Same,
    Append(String),
    Replace(String),
}

/// What a read found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// A file with the output, how far it is read now and what the read adds.
    File { path: PathBuf, read: u64, change: Change },
    /// Output the agent handed over, whole as far as it keeps it.
    Text(String),
    /// Nothing yet.
    Nothing,
}

/// What `text` changes when it follows `shown`, both of the same origin.
pub(crate) fn change(shown: &str, text: &str) -> Change {
    if text == shown {
        Change::Same
    } else if let Some(added) = text.strip_prefix(shown).filter(|_| !shown.is_empty()) {
        Change::Append(added.to_string())
    } else {
        Change::Replace(text.to_string())
    }
}

/// Reads the output of a task from the first source that has some: the file the agent writes it to, `text` the agent
/// handed over, the last of `logs` the command redirects its output to that is there. `file` is the file shown and how
/// far it is read, so a file read again adds only its new end. It reads the disk.
pub(crate) fn find(agent_file: Option<&Path>, text: String, logs: &[PathBuf], file: Option<(&Path, u64)>) -> Found {
    let filled = agent_file.filter(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0));
    let path = match filled {
        Some(path) => path,
        None if !text.is_empty() => return Found::Text(text),
        None => match logs.iter().rev().find(|path| path.is_file()) {
            Some(path) => path,
            None => return Found::Nothing,
        },
    };
    let from = file.filter(|(shown, _)| *shown == path).map(|(_, read)| read);
    match read_from(path, from) {
        Ok((read, change)) => Found::File {
            path: path.to_path_buf(),
            read,
            change,
        },
        Err(_) => Found::Nothing,
    }
}

/// Reads `path` on from byte `from`, where an earlier read ended; the end of the file from scratch without one, after
/// the file shrank, or when more came than a tab shows. How far it read and what it adds, without a character cut in
/// half at the end.
pub(crate) fn read_from(path: &Path, from: Option<u64>) -> std::io::Result<(u64, Change)> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let from = from.filter(|&from| from <= length && length - from <= MAX_SHOWN);
    if from == Some(length) {
        return Ok((length, Change::Same));
    }
    let start = from.unwrap_or_else(|| length.saturating_sub(MAX_SHOWN));
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(length - start).read_to_end(&mut bytes)?;
    let whole = match std::str::from_utf8(&bytes) {
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        _ => bytes.len(),
    };
    let read = start + whole as u64;
    let mut text = String::from_utf8_lossy(&bytes[..whole]).into_owned();
    if from.is_some() {
        return Ok((read, Change::Append(text)));
    }
    if start > 0 {
        let line = text.find('\n').map_or(0, |at| at + 1);
        text.replace_range(..line, "…\n");
    }
    Ok((read, Change::Replace(text)))
}

/// The command line of background task `item`, from the tool call that started it.
pub(crate) fn command_line(thread: &Thread, item: &BackgroundItem) -> Option<String> {
    match &thread.entry_by_key(item.tool_call.as_deref()?)?.kind {
        EntryKind::ToolCall(call) => match &call.view {
            ToolView::Command { command, .. } => Some(command.clone()),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agent-task-output-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn handed_over_output_appends_what_follows_and_replaces_what_moved() {
        assert_eq!(change("a\n", "a\n"), Change::Same);
        assert_eq!(change("a\n", "a\nb\n"), Change::Append("b\n".into()));
        assert_eq!(change("a\nb\n", "b\nc\n"), Change::Replace("b\nc\n".into()));
        assert_eq!(change("", "a"), Change::Replace("a".into()));
    }

    #[test]
    fn a_growing_file_is_read_on_from_where_the_last_read_ended() {
        let dir = folder("grow");
        let log = dir.join("build.log");
        std::fs::write(&log, "one\n").unwrap();
        let (read, first) = read_from(&log, None).unwrap();
        assert_eq!((read, first), (4, Change::Replace("one\n".into())));
        assert_eq!(read_from(&log, Some(read)).unwrap(), (4, Change::Same));
        std::fs::write(&log, "one\ntwo ä").unwrap();
        let mut bytes = std::fs::read(&log).unwrap();
        bytes.pop();
        std::fs::write(&log, &bytes).unwrap();
        assert_eq!(read_from(&log, Some(4)).unwrap(), (8, Change::Append("two ".into())));
        std::fs::write(&log, "new\n").unwrap();
        assert_eq!(read_from(&log, Some(8)).unwrap(), (4, Change::Replace("new\n".into())));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_longer_than_a_tab_shows_starts_at_a_line_of_its_end() {
        let dir = folder("long");
        let log = dir.join("long.log");
        let line = "x".repeat(99) + "\n";
        std::fs::write(&log, line.repeat(usize::try_from(MAX_SHOWN / 100 + 5).unwrap())).unwrap();
        let (read, change) = read_from(&log, None).unwrap();
        assert_eq!(read, std::fs::metadata(&log).unwrap().len());
        let Change::Replace(text) = change else {
            panic!("{change:?}")
        };
        assert!(text.starts_with("…\nxxx"));
        assert!(text.len() as u64 <= MAX_SHOWN + 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_agents_file_comes_first_then_its_text_then_the_last_log_the_command_wrote() {
        let dir = folder("find");
        let agent = dir.join("task.out");
        let log = dir.join("batch.log");
        let missing = dir.join("missing.log");
        let logs = [log.clone(), missing.clone()];
        assert_eq!(find(Some(&agent), String::new(), &logs, None), Found::Nothing);
        std::fs::write(&log, "tick\n").unwrap();
        assert_eq!(
            find(Some(&agent), "text".into(), &logs, None),
            Found::Text("text".into())
        );
        let from_log = find(None, String::new(), &logs, None);
        assert_eq!(
            from_log,
            Found::File {
                path: log.clone(),
                read: 5,
                change: Change::Replace("tick\n".into())
            }
        );
        assert!(matches!(
            find(None, String::new(), &logs, Some((&log, 5))),
            Found::File {
                change: Change::Same,
                ..
            }
        ));
        std::fs::write(&agent, "all\n").unwrap();
        assert!(matches!(
            find(Some(&agent), "text".into(), &logs, Some((&log, 5))),
            Found::File {
                read: 4,
                change: Change::Replace(_),
                ..
            }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_background_task_reads_its_command_from_the_call_that_started_it_also_in_a_subagent() {
        let mut thread = Thread::new(term_thread::ThreadId::from("t"), "test", "/tmp");
        let mut call = term_thread::ToolCall::new("toolu_bg", "Bash", term_thread::ToolKind::Execute);
        call.view = ToolView::Command {
            command: "./batch.sh > batch.log 2>&1".into(),
            description: None,
            background: true,
            timeout: None,
        };
        thread.apply(term_thread::Update::Put {
            lane: term_thread::Lane::Subagent("toolu_agent".into()),
            key: Some("toolu_bg".into()),
            at: std::time::UNIX_EPOCH,
            entry: EntryKind::ToolCall(call),
        });
        let item = |tool_call: Option<&str>| BackgroundItem {
            id: "b1".into(),
            kind: term_thread::BackgroundKind::Shell,
            label: "Batch".into(),
            status: term_thread::TaskStatus::Running,
            started: None,
            ended: None,
            tool_call: tool_call.map(str::to_string),
            parent: None,
            output: term_thread::OutputTail::default(),
            is_backgrounded: true,
            ambient: false,
        };
        assert_eq!(
            command_line(&thread, &item(Some("toolu_bg"))).as_deref(),
            Some("./batch.sh > batch.log 2>&1")
        );
        assert_eq!(command_line(&thread, &item(Some("toolu_other"))), None);
        assert_eq!(command_line(&thread, &item(None)), None);
    }
}
