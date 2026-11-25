//! Past sessions from the CLI's own transcripts in `~/.claude/projects`; experimental, because the format is the
//! CLI's internal one. Every function tolerates files it cannot read and leaves them out.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use serde_json::Value;
use term_base::LockIgnoringPoison;

use term_thread::Thread;

use crate::mapping::Mapper;
use crate::protocol::{Message, System};
use crate::tools;

/// How many lines from the start of a transcript are searched for the first prompt.
const HEAD_LINES: usize = 400;
/// How many bytes from the end of a transcript are searched for titles, which the CLI appends.
const TAIL_BYTES: u64 = 256 * 1024;

/// A session the CLI saved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct PastSession {
    /// Its id, which resuming names.
    pub id: String,
    /// The title the user set or the CLI generated.
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    /// The latest prompt.
    pub last_prompt: Option<String>,
    /// The git branch it began on.
    pub git_branch: Option<String>,
    /// When the transcript last changed.
    pub modified: SystemTime,
    /// The transcript file.
    pub path: PathBuf,
}

impl PastSession {
    /// What a list calls it, on one line: the title, else the first prompt's first line that is not blank.
    pub fn heading(&self) -> Option<String> {
        [&self.title, &self.first_prompt]
            .into_iter()
            .flatten()
            .map(|text| term_thread::title_from(text))
            .find(|line| !line.is_empty())
    }
}

/// The CLI's configuration folder: `CLAUDE_CONFIG_DIR` from `env`, else `~/.claude`.
pub fn config_dir(env: &[(String, String)]) -> PathBuf {
    env.iter()
        .find(|(name, value)| name == "CLAUDE_CONFIG_DIR" && !value.is_empty())
        .map_or_else(|| term_base::home().join(".claude"), |(_, value)| PathBuf::from(value))
}

/// The folder name the CLI files the transcripts of `cwd` under: every character but letters and digits becomes `-`.
pub fn project_folder(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A transcript's summary as it was read, with the length and time of change the file had then.
type Summaries = HashMap<PathBuf, (u64, SystemTime, Option<PastSession>)>;

/// The summaries read so far, by folder, so that a listing reads only the transcripts that changed since.
static SUMMARIES: LazyLock<Mutex<HashMap<PathBuf, Summaries>>> = LazyLock::new(Mutex::default);

/// The sessions saved for `cwd`, most recent first.
pub fn list(config_dir: &Path, cwd: &Path) -> Vec<PastSession> {
    let folder = config_dir.join("projects").join(project_folder(cwd));
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Vec::new();
    };
    let mut known = SUMMARIES.lock_ignoring_poison().remove(&folder).unwrap_or_default();
    let mut read = Summaries::new();
    for path in entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
    {
        let Some((length, modified)) = std::fs::metadata(&path)
            .ok()
            .and_then(|m| Some((m.len(), m.modified().ok()?)))
        else {
            continue;
        };
        let summary = match known.remove(&path) {
            Some((was, at, summary)) if was == length && at == modified => summary,
            _ => read_summary(&path),
        };
        read.insert(path, (length, modified, summary));
    }
    let mut sessions: Vec<PastSession> = read.values().filter_map(|(_, _, s)| s.clone()).collect();
    SUMMARIES.lock_ignoring_poison().insert(folder, read);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
    sessions
}

/// The session `id` saved for `cwd` as `list` has it, read from its transcript alone.
pub fn find(config_dir: &Path, cwd: &Path, id: &str) -> Option<PastSession> {
    if id.is_empty() || id.contains('/') {
        return None;
    }
    let path = config_dir
        .join("projects")
        .join(project_folder(cwd))
        .join(format!("{id}.jsonl"));
    path.is_file().then(|| read_summary(&path)).flatten()
}

fn read_summary(path: &Path) -> Option<PastSession> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let mut file = File::open(path).ok()?;
    let modified = file.metadata().ok()?.modified().ok()?;
    let mut session = PastSession {
        id,
        title: None,
        first_prompt: None,
        last_prompt: None,
        git_branch: None,
        modified,
        path: path.to_path_buf(),
    };
    for line in BufReader::new(&file).lines().take(HEAD_LINES).map_while(Result::ok) {
        // Answers and tool results, which can run to megabytes, add nothing here and are not parsed.
        let wanted = (session.git_branch.is_none() && line.contains("\"gitBranch\""))
            || (session.first_prompt.is_none() && line.contains("\"user\""))
            || names_a_title(&line);
        if !wanted {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if session.git_branch.is_none() {
            session.git_branch = text(&record, "gitBranch");
        }
        if session.first_prompt.is_none() && is_prompt(&record) {
            session.first_prompt = prompt_text(&record);
        }
        learn_titles(&record, &mut session);
    }
    let length = file.metadata().ok()?.len();
    if length > 0 && file.seek(SeekFrom::Start(length.saturating_sub(TAIL_BYTES))).is_ok() {
        let mut tail = Vec::new();
        let _ = file.read_to_end(&mut tail);
        for line in String::from_utf8_lossy(&tail)
            .lines()
            .filter(|line| names_a_title(line))
        {
            if let Ok(record) = serde_json::from_str::<Value>(line) {
                learn_titles(&record, &mut session);
            }
        }
    }
    (session.first_prompt.is_some() || session.title.is_some()).then_some(session)
}

/// Whether a transcript line may be one of the records `learn_titles` reads, judged by its text alone.
fn names_a_title(line: &str) -> bool {
    ["\"custom-title\"", "\"ai-title\"", "\"summary\"", "\"last-prompt\""]
        .iter()
        .any(|kind| line.contains(kind))
}

fn text(record: &Value, field: &str) -> Option<String> {
    record
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn learn_titles(record: &Value, session: &mut PastSession) {
    match record.get("type").and_then(Value::as_str) {
        Some("custom-title") => session.title = text(record, "customTitle").or(session.title.take()),
        Some("ai-title") if session.title.is_none() => session.title = text(record, "aiTitle"),
        Some("summary") if session.title.is_none() => session.title = text(record, "summary"),
        Some("last-prompt") => session.last_prompt = text(record, "lastPrompt").or(session.last_prompt.take()),
        _ => {}
    }
}

/// Whether `record` is a prompt the user typed in the main conversation.
fn is_prompt(record: &Value) -> bool {
    record.get("type").and_then(Value::as_str) == Some("user")
        && record.get("isSidechain").and_then(Value::as_bool) != Some(true)
        && record.get("isMeta").and_then(Value::as_bool) != Some(true)
        && prompt_text(record).is_some()
}

fn prompt_text(record: &Value) -> Option<String> {
    let content = record.pointer("/message/content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !text.starts_with('<')).then(|| text.to_string())
}

/// The main conversation of a transcript as frames, read one at a time, for `Mapper::for_history`; records it cannot
/// read are skipped.
pub(crate) fn frames(path: &Path) -> std::io::Result<impl Iterator<Item = Message>> {
    let file = File::open(path)?;
    Ok(BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|record| {
            record.get("isSidechain").and_then(Value::as_bool) != Some(true)
                && record.get("isMeta").and_then(Value::as_bool) != Some(true)
        })
        .filter_map(frame)
        .filter(|message| !matches!(message, Message::Unknown(_) | Message::Malformed(_))))
}

/// The frame a transcript record stands for: a slash command as the user typed it, and its output as the CLI sent
/// it while the session ran.
fn frame(mut record: Value) -> Option<Message> {
    match record.get("type").and_then(Value::as_str)? {
        "user" => {
            match record.get_mut("toolUseResult") {
                Some(Value::Object(result)) => result.retain(|field, _| tools::RESULT_FIELDS.contains(&field.as_str())),
                Some(other) => *other = Value::Null,
                None => {}
            }
            let typed = record
                .pointer("/message/content")
                .and_then(Value::as_str)
                .and_then(command);
            if let (Some(typed), Some(message)) = (typed, record.get_mut("message")) {
                message["content"] = Value::String(typed);
            }
            Some(Message::from_value(record))
        }
        "assistant" => Some(Message::from_value(record)),
        // A task's end that arrived while a turn ran: the transcript keeps it as a queued command, not as a message.
        "attachment"
            if record.pointer("/attachment/commandMode").and_then(Value::as_str) == Some("task-notification") =>
        {
            let prompt = record.pointer("/attachment/prompt").and_then(Value::as_str)?;
            Some(Message::from_value(serde_json::json!({
                "type": "user",
                "isReplay": true,
                "origin": {"kind": "task-notification"},
                "message": {"role": "user", "content": prompt},
            })))
        }
        "system" if record.get("subtype").and_then(Value::as_str) == Some("local_command") => {
            let content = record.get("content").and_then(Value::as_str)?;
            let output = between(content, "<local-command-stdout>", "</local-command-stdout>")
                .or_else(|| between(content, "<local-command-stderr>", "</local-command-stderr>"))?
                .trim();
            (!output.is_empty()).then(|| Message::System(System::LocalCommandOutput(output.to_string())))
        }
        _ => None,
    }
}

/// `/name args` of a slash command, which the transcript keeps as `<command-name>` and `<command-args>` tags.
fn command(content: &str) -> Option<String> {
    let name = between(content, "<command-name>", "</command-name>")?;
    let args = between(content, "<command-args>", "</command-args>").unwrap_or_default();
    Some(format!("{name} {args}").trim().to_string())
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    Some(&text[start..end])
}

/// `thread` with the main conversation of the transcript at `path`, to show before resuming the session.
pub fn rebuild(mut thread: Thread, path: &Path) -> std::io::Result<Thread> {
    let at = std::fs::metadata(path)?.modified()?;
    let mut mapper = Mapper::for_history();
    for message in frames(path)? {
        let updates = mapper.map(&message, at);
        thread.apply_all(updates);
    }
    Ok(thread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_base::testing::scratch;

    #[test]
    fn the_project_folder_replaces_every_separator() {
        assert_eq!(
            project_folder(Path::new("/private/tmp/claude-501/-Users-someone/a.b_c")),
            "-private-tmp-claude-501--Users-someone-a-b-c"
        );
    }

    #[test]
    fn sessions_are_listed_with_prompt_title_and_branch() {
        let config = scratch("list");
        let cwd = Path::new("/work/demo");
        let folder = config.join("projects").join(project_folder(cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let transcript = [
            r#"{"type":"session-owner","sessionId":"s1"}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"<command-name>/x</command-name>"}}"#,
            r#"{"type":"user","uuid":"u1","gitBranch":"main","isSidechain":false,"message":{"role":"user","content":[{"type":"text","text":"Fix the parser"}]}}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
            "not json",
            r#"{"type":"ai-title","aiTitle":"Parser fix","sessionId":"s1"}"#,
            r#"{"type":"last-prompt","lastPrompt":"Thanks","sessionId":"s1"}"#,
        ];
        std::fs::write(folder.join("s1.jsonl"), transcript.join("\n")).unwrap();
        std::fs::write(folder.join("empty.jsonl"), "").unwrap();
        std::fs::write(folder.join("notes.txt"), "x").unwrap();
        let sessions = list(&config, cwd);
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s.id, "s1");
        assert_eq!(s.first_prompt.as_deref(), Some("Fix the parser"));
        assert_eq!(s.title.as_deref(), Some("Parser fix"));
        assert_eq!(s.last_prompt.as_deref(), Some("Thanks"));
        assert_eq!(s.git_branch.as_deref(), Some("main"));
        assert_eq!(
            frames(&s.path).unwrap().count(),
            2,
            "the meta record and the unreadable line are left out"
        );
        let thread = rebuild(Thread::new(term_thread::ThreadId::from("t"), "claude", cwd), &s.path).unwrap();
        let texts: Vec<String> = thread
            .entries(&term_thread::Lane::Main)
            .iter()
            .map(|e| match &e.kind {
                term_thread::EntryKind::UserMessage(m) => m.text.clone(),
                term_thread::EntryKind::AgentMessage(m) => m.markdown.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, ["Fix the parser", "Done."]);
        let frames: Vec<(Option<&str>, Option<&str>)> = thread
            .entries(&term_thread::Lane::Main)
            .iter()
            .map(|e| (e.uuid.as_deref(), e.follows.as_deref()))
            .collect();
        assert_eq!(frames, [(Some("u1"), None), (Some("a1"), Some("u1"))]);
    }

    #[test]
    fn a_background_agent_ends_where_its_end_came_during_a_turn_without_its_call() {
        let folder = scratch("async");
        let path = folder.join("s3.jsonl");
        let transcript = [
            r#"{"type":"assistant","uuid":"a1","message":{"id":"m1","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_bg","name":"Agent","input":{"description":"review code","subagent_type":"general-purpose","prompt":"Review","run_in_background":true}}]}}"#,
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bg","content":"Async agent launched successfully."}]},"toolUseResult":{"isAsync":true,"status":"async_launched","agentId":"agent7"}}"#,
            r#"{"type":"attachment","uuid":"q1","attachment":{"type":"queued_command","commandMode":"task-notification","prompt":"<task-notification>\n<task-id>agent7</task-id>\n<status>completed</status>\n<summary>Agent finished</summary>\n<usage><subagent_tokens>10</subagent_tokens><tool_uses>2</tool_uses><duration_ms>3000</duration_ms></usage>\n</task-notification>"}}"#,
        ];
        std::fs::write(&path, transcript.join("\n")).unwrap();
        let thread = rebuild(Thread::new(term_thread::ThreadId::from("t"), "claude", &folder), &path).unwrap();
        let agent = thread.subagent("toolu_bg").expect("the call starts a subagent");
        assert_eq!(agent.status, term_thread::TaskStatus::Completed);
        assert_eq!(agent.task_id.as_deref(), Some("agent7"));
        assert_eq!((agent.tokens, agent.elapsed), (10, std::time::Duration::from_secs(3)));
        assert!(
            !thread
                .entries(&term_thread::Lane::Main)
                .iter()
                .any(|e| matches!(e.kind, term_thread::EntryKind::UserMessage(_))),
            "the notification is no message of the user"
        );
    }

    #[test]
    fn a_slash_command_comes_back_as_typed_with_its_output() {
        let folder = scratch("command");
        let path = folder.join("s2.jsonl");
        let transcript = [
            r#"{"type":"user","isMeta":true,"uuid":"c0","message":{"role":"user","content":"<local-command-caveat>The command below was run directly.</local-command-caveat>"}}"#,
            r#"{"type":"user","uuid":"c1","message":{"role":"user","content":"<command-name>/mcp</command-name>\n            <command-message>mcp</command-message>\n            <command-args></command-args>"}}"#,
            r#"{"type":"system","subtype":"local_command","isMeta":false,"level":"info","uuid":"c2","content":"<local-command-stdout>8 MCP server(s): 5 connected.</local-command-stdout>"}"#,
            r#"{"type":"user","uuid":"c3","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"}}"#,
            r#"{"type":"system","subtype":"local_command","uuid":"c4","content":"<local-command-stdout></local-command-stdout>"}"#,
        ];
        std::fs::write(&path, transcript.join("\n")).unwrap();
        let thread = rebuild(Thread::new(term_thread::ThreadId::from("t"), "claude", &folder), &path).unwrap();
        let texts: Vec<String> = thread
            .entries(&term_thread::Lane::Main)
            .iter()
            .map(|e| match &e.kind {
                term_thread::EntryKind::UserMessage(m) => format!("user: {}", m.text),
                term_thread::EntryKind::AgentMessage(m) => format!("output: {}", m.markdown),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            texts,
            [
                "user: /mcp",
                "output: 8 MCP server(s): 5 connected.",
                "user: /model opus"
            ]
        );
    }

    /// A picture's type and bytes.
    type Picture = (String, Vec<u8>);

    #[test]
    fn a_pasted_picture_comes_back_with_its_message() {
        let folder = scratch("picture");
        let path = folder.join("s3.jsonl");
        let transcript = [
            r#"{"type":"user","uuid":"p1","message":{"role":"user","content":[{"type":"text","text":"Look"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw=="}}]}}"#,
            r#"{"type":"user","uuid":"p2","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"/9j/"}}]}}"#,
        ];
        std::fs::write(&path, transcript.join("\n")).unwrap();
        let thread = rebuild(Thread::new(term_thread::ThreadId::from("t"), "claude", &folder), &path).unwrap();
        let pictures: Vec<(String, Vec<Picture>)> = thread
            .entries(&term_thread::Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                term_thread::EntryKind::UserMessage(m) => Some((
                    m.text.clone(),
                    m.attachments
                        .iter()
                        .filter_map(|a| match a {
                            term_thread::Attachment::Image { media_type, data, .. } => {
                                Some((media_type.clone(), data.to_vec()))
                            }
                            _ => None,
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            pictures,
            [
                (
                    "Look".to_string(),
                    vec![("image/png".to_string(), vec![0x89, b'P', b'N', b'G'])]
                ),
                (String::new(), vec![("image/jpeg".to_string(), vec![0xff, 0xd8, 0xff])]),
            ]
        );
    }

    #[test]
    fn a_listing_follows_transcripts_that_change_appear_and_go() {
        let config = scratch("list-again");
        let cwd = Path::new("/work/again");
        let folder = config.join("projects").join(project_folder(cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let prompt = |text: &str| {
            format!(
                r#"{{"type":"user","uuid":"u","message":{{"role":"user","content":[{{"type":"text","text":"{text}"}}]}}}}"#
            )
        };
        std::fs::write(folder.join("a.jsonl"), prompt("First")).unwrap();
        let titles = |sessions: Vec<PastSession>| -> Vec<(String, Option<String>)> {
            let mut titles: Vec<_> = sessions.into_iter().map(|s| (s.id, s.title)).collect();
            titles.sort();
            titles
        };
        assert_eq!(titles(list(&config, cwd)), [("a".to_string(), None)]);
        let renamed = format!(
            "{}\n{}",
            prompt("First"),
            r#"{"type":"custom-title","customTitle":"Renamed"}"#
        );
        std::fs::write(folder.join("a.jsonl"), renamed).unwrap();
        std::fs::write(folder.join("b.jsonl"), prompt("Second")).unwrap();
        assert_eq!(
            titles(list(&config, cwd)),
            [("a".to_string(), Some("Renamed".to_string())), ("b".to_string(), None)]
        );
        std::fs::remove_file(folder.join("a.jsonl")).unwrap();
        assert_eq!(titles(list(&config, cwd)), [("b".to_string(), None)]);
    }

    #[test]
    fn one_session_is_found_as_the_listing_has_it_without_reading_the_others() {
        let config = scratch("find");
        let cwd = Path::new("/work/find");
        let folder = config.join("projects").join(project_folder(cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let prompt = r#"{"type":"user","uuid":"u","message":{"role":"user","content":[{"type":"text","text":"Hi"}]}}"#;
        std::fs::write(folder.join("a.jsonl"), prompt).unwrap();
        std::fs::write(folder.join("blank.jsonl"), "").unwrap();
        let listed = list(&config, cwd).into_iter().find(|s| s.id == "a");
        assert_eq!(find(&config, cwd, "a"), listed);
        assert!(find(&config, cwd, "a").is_some());
        assert_eq!(find(&config, cwd, "blank"), None, "the listing leaves it out too");
        assert_eq!(find(&config, cwd, "missing"), None);
        assert_eq!(find(&config, cwd, "../find/a"), None);
    }

    #[test]
    fn records_with_nothing_for_the_list_are_skipped_without_losing_one_that_has_something() {
        let folder = scratch("prefilter");
        let path = folder.join("s4.jsonl");
        let answer = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            "x".repeat(64 * 1024)
        );
        let transcript = [
            answer.as_str(),
            r#"{"type":"attachment","gitBranch":"dev"}"#,
            r#"{"type":"user","message":{"role":"user","content":"First prompt"}}"#,
            r#"{"type":"summary","summary":"A summary"}"#,
            r#"{"type":"last-prompt","lastPrompt":"Last"}"#,
        ];
        std::fs::write(&path, transcript.join("\n")).unwrap();
        let session = read_summary(&path).unwrap();
        assert_eq!(session.git_branch.as_deref(), Some("dev"));
        assert_eq!(session.first_prompt.as_deref(), Some("First prompt"));
        assert_eq!(session.title.as_deref(), Some("A summary"));
        assert_eq!(session.last_prompt.as_deref(), Some("Last"));
        assert!(names_a_title(r#"{"type":"custom-title"}"#));
        assert!(!names_a_title(&answer));
    }

    #[test]
    fn a_missing_folder_lists_nothing() {
        assert!(list(&scratch("missing"), Path::new("/nowhere")).is_empty());
    }

    #[test]
    fn the_config_folder_follows_the_environment() {
        let env = vec![("CLAUDE_CONFIG_DIR".to_string(), "/custom".to_string())];
        assert_eq!(config_dir(&env), PathBuf::from("/custom"));
        assert!(config_dir(&[]).ends_with(".claude"));
    }

    #[test]
    fn a_heading_is_one_line_from_the_title_or_the_first_prompt() {
        let session = |title: Option<&str>, prompt: Option<&str>| PastSession {
            id: "s".into(),
            title: title.map(Into::into),
            first_prompt: prompt.map(Into::into),
            last_prompt: None,
            git_branch: None,
            modified: SystemTime::UNIX_EPOCH,
            path: PathBuf::new(),
        };
        let two_lines = Some("\nWhy is the list off?\nMove the age right");
        assert_eq!(
            session(None, two_lines).heading().as_deref(),
            Some("Why is the list off?")
        );
        assert_eq!(
            session(Some("Parser fix"), two_lines).heading().as_deref(),
            Some("Parser fix")
        );
        assert_eq!(
            session(Some("  "), two_lines).heading().as_deref(),
            Some("Why is the list off?")
        );
        assert_eq!(session(None, None).heading(), None);
    }
}
