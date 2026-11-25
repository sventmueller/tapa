//! What the agent tabs keep of past sessions without Dango's Agents panel: the thread index,
//! the bookmarks and the past sessions of each base path, which the tab's menu and the restored state read.

mod store;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::SystemTime;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSColor, NSTextField, NSView};
use term_thread::ThreadIndex;

use crate::Resume;
use crate::common::util::ns;
pub(crate) use store::{agent_id_of, past_sessions_cached, read_past, record, rename, set_bookmarked};
pub use store::{is_bookmarked, past_session, wait_for_writes};

/// What the main thread keeps of the index and the bookmarks.
#[derive(Default)]
pub(crate) struct AgentPanels {
    store: store::Store,
}

fn with_state<R>(f: impl FnOnce(&AgentPanels) -> R) -> R {
    f(&crate::host::state().agent_panels)
}

/// Shows that the sessions changed: the sidebar reads the workspaces' work again.
fn reload_all() {
    for tab in crate::all() {
        if let Some(controller) = tab.controller() {
            term_ui::workspace_host::Notices::refresh_activity(&controller);
        }
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Flipped` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentFlippedView"]
    pub(crate) struct Flipped;

    impl Flipped {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

/// A view whose origin is at its top, as the document of a scroll view that starts at the top.
pub(crate) fn flipped(mtm: MainThreadMarker) -> Retained<NSView> {
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let view: Retained<Flipped> = unsafe { msg_send![Flipped::alloc(mtm), init] };
    Retained::into_super(view)
}

/// The head of a section, in small capitals as the sidebar's sections read.
pub(crate) fn section_label(mtm: MainThreadMarker, title: &str) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&ns(&title.to_uppercase()), mtm);
    label.setFont(Some(&crate::style::bold(10.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

fn bookmarks_file() -> std::path::PathBuf {
    ThreadIndex::default_dir().join("bookmarks.json")
}

fn bookmarks() -> BTreeSet<String> {
    bookmarks_in(&bookmarks_file())
}

fn bookmarks_in(file: &Path) -> BTreeSet<String> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes the bookmarks `marks`.
fn write_bookmarks(marks: &BTreeSet<String>) -> std::io::Result<()> {
    write_bookmarks_to(&bookmarks_file(), marks)
}

fn write_bookmarks_to(file: &Path, marks: &BTreeSet<String>) -> std::io::Result<()> {
    term_base::atomic_write::write(file, &serde_json::to_vec(marks).unwrap_or_default())
}

/// The sessions of `cwd` that can be resumed, newest first: the index's, then — experimental — those the CLI saved
/// from a terminal. Reads the disk; the main thread takes `past_sessions_cached`.
pub(crate) fn past_sessions(cwd: &Path, limit: usize) -> Vec<(Resume, SystemTime)> {
    let index = ThreadIndex::open(&ThreadIndex::default_dir());
    let mut past: Vec<(Resume, SystemTime)> = index
        .list(&term_thread::Filter::default())
        .into_iter()
        .filter(|e| e.cwd.as_deref() == Some(cwd))
        .filter_map(|e| {
            let session = e.session.as_deref()?;
            Some((indexed(e, session, transcript(cwd, session)), e.last_activity))
        })
        .collect();
    let known: BTreeSet<String> = past.iter().map(|(r, _)| r.session.clone()).collect();
    for session in terminal_sessions(cwd) {
        if known.contains(&session.id) {
            continue;
        }
        let modified = session.modified;
        past.push((from_terminal(session), modified));
    }
    let known: BTreeSet<String> = past.iter().map(|(r, _)| r.session.clone()).collect();
    past.extend(
        crate::codex::past_threads(cwd)
            .into_iter()
            .filter(|(r, _)| !known.contains(&r.session)),
    );
    past.sort_by_key(|(_, when)| std::cmp::Reverse(*when));
    past.truncate(limit);
    past
}

/// The session `session` of `cwd` as `past_sessions` lists it, read without listing the others.
pub(crate) fn find_past_session(index: &ThreadIndex, cwd: &Path, session: &str) -> Option<Resume> {
    find_past_in(index, &config_dir(), cwd, session, || crate::codex::past_threads(cwd))
}

/// `find_past_session` with the CLI's configuration folder `config` and the folder's Codex threads from `codex`.
fn find_past_in(
    index: &ThreadIndex,
    config: &Path,
    cwd: &Path,
    session: &str,
    codex: impl FnOnce() -> Vec<(Resume, SystemTime)>,
) -> Option<Resume> {
    let entry = index
        .list(&term_thread::Filter::default())
        .into_iter()
        .find(|e| e.cwd.as_deref() == Some(cwd) && e.session.as_deref() == Some(session));
    if let Some(entry) = entry {
        return Some(indexed(entry, session, transcript_in(config, cwd, session)));
    }
    if let Some(past) = term_claude::history::find(config, cwd, session) {
        return Some(from_terminal(past));
    }
    codex()
        .into_iter()
        .filter(|(resume, _)| resume.session == session)
        .min_by_key(|(_, when)| std::cmp::Reverse(*when))
        .map(|(resume, _)| resume)
}

fn indexed(entry: &term_thread::IndexEntry, session: &str, transcript: Option<std::path::PathBuf>) -> Resume {
    Resume {
        transcript,
        session: session.to_string(),
        thread: Some(entry.id.clone()),
        title: Some(entry.title.clone()).filter(|t| !t.is_empty()),
        agent: None,
    }
}

fn from_terminal(session: term_claude::history::PastSession) -> Resume {
    Resume {
        title: session.heading(),
        session: session.id,
        thread: None,
        transcript: Some(session.path),
        agent: None,
    }
}

fn config_dir() -> std::path::PathBuf {
    term_claude::history::config_dir(&term_ui::workspace_host::environment(std::time::Duration::ZERO))
}

/// The sessions the CLI saved for `cwd`, as its own session list shows them; experimental.
fn terminal_sessions(cwd: &Path) -> Vec<term_claude::history::PastSession> {
    term_claude::history::list(&config_dir(), cwd)
}

fn transcript(cwd: &Path, session: &str) -> Option<std::path::PathBuf> {
    transcript_in(&config_dir(), cwd, session)
}

fn transcript_in(config: &Path, cwd: &Path, session: &str) -> Option<std::path::PathBuf> {
    if session.contains('/') {
        return None;
    }
    let path = config
        .join("projects")
        .join(term_claude::history::project_folder(cwd))
        .join(format!("{session}.jsonl"));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_replace_the_file_whole_and_leave_no_scratch_file() {
        let dir = term_base::testing::scratch("bookmarks");
        let file = dir.path().join("threads").join("bookmarks.json");
        let first: BTreeSet<String> = ["a".to_string(), "b".to_string()].into();
        write_bookmarks_to(&file, &first).unwrap();
        assert_eq!(bookmarks_in(&file), first);
        let second: BTreeSet<String> = ["c".to_string()].into();
        write_bookmarks_to(&file, &second).unwrap();
        assert_eq!(bookmarks_in(&file), second);
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["bookmarks.json"]);
    }

    #[test]
    fn one_past_session_is_found_where_the_listing_would_take_it_from() {
        let dir = term_base::testing::scratch("find-past");
        let cwd = Path::new("/work/find-past");
        let config = dir.path().join("config");
        let folder = config.join("projects").join(term_claude::history::project_folder(cwd));
        std::fs::create_dir_all(&folder).unwrap();
        let prompt = r#"{"type":"user","uuid":"u","message":{"role":"user","content":[{"type":"text","text":"From a terminal"}]}}"#;
        for session in ["indexed", "terminal"] {
            std::fs::write(folder.join(format!("{session}.jsonl")), prompt).unwrap();
        }
        let mut index = ThreadIndex::open(&dir.path().join("threads"));
        let entry = |id: &str, session: &str, archived: bool| term_thread::IndexEntry {
            id: term_thread::ThreadId::from(id),
            title: "Kept".into(),
            agent: "claude".into(),
            group: None,
            archived,
            last_activity: SystemTime::UNIX_EPOCH,
            status: term_thread::Status::default(),
            session: Some(session.into()),
            cwd: Some(cwd.to_path_buf()),
            cost_usd: None,
        };
        index.upsert(entry("t1", "indexed", false)).unwrap();
        index.upsert(entry("t2", "archived", true)).unwrap();
        let codex = || {
            let thread = Resume {
                session: "codex".into(),
                thread: None,
                title: Some("Codex".into()),
                transcript: None,
                agent: Some("codex".into()),
            };
            vec![(thread, SystemTime::UNIX_EPOCH)]
        };
        let find = |session: &str| find_past_in(&index, &config, cwd, session, codex);

        let indexed = find("indexed").unwrap();
        assert_eq!(indexed.thread, Some(term_thread::ThreadId::from("t1")));
        assert_eq!(indexed.title.as_deref(), Some("Kept"));
        assert_eq!(indexed.transcript, Some(folder.join("indexed.jsonl")));
        let terminal = find("terminal").unwrap();
        assert_eq!(
            (terminal.thread, terminal.title),
            (None, Some("From a terminal".into()))
        );
        assert_eq!(terminal.transcript, Some(folder.join("terminal.jsonl")));
        assert_eq!(find("codex").unwrap().agent.as_deref(), Some("codex"));
        assert!(find("archived").is_none(), "the listing leaves archived threads out");
        assert!(find("missing").is_none());
    }
}
