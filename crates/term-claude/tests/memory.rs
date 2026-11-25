//! The memory a long Claude session takes when it is shown again from its transcript (E25): while it is read and
//! once its timeline stands.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use term_base::testing::scratch;
use term_thread::{Lane, Thread, ThreadId};

struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    let held = HELD.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(held, Ordering::Relaxed);
}

// SAFETY: Every call is passed on to the system allocator unchanged; only the byte counts are added up.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: The caller's guarantees for `layout` hold for the system allocator too.
        let block = unsafe { System.alloc(layout) };
        if !block.is_null() {
            grew(layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // SAFETY: `block` came from `alloc` or `realloc` above, which handed out the system allocator's blocks.
        unsafe { System.dealloc(block, layout) };
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: As for `dealloc`; the caller vouches for `size`.
        let moved = unsafe { System.realloc(block, layout, size) };
        if !moved.is_null() {
            HELD.fetch_sub(layout.size(), Ordering::Relaxed);
            grew(size);
        }
        moved
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// How many turns the transcript has; each reads a file, edits one, runs a command and asks a browser for its page.
const TURNS: usize = 120;

/// `count` lines of about `width` characters, numbered so that no two are alike.
fn lines(count: usize, width: usize, tag: &str) -> String {
    let mut text = String::new();
    for n in 0..count {
        let _ = writeln!(text, "{tag} {n:05} {}", "x".repeat(width.saturating_sub(tag.len() + 7)));
    }
    text
}

fn record(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect("a record is JSON")
}

/// One turn's transcript records, as the CLI writes them.
fn turn(n: usize) -> Vec<String> {
    let file = format!("/work/src/file{n}.rs");
    let read = lines(800, 60, "read");
    let original = lines(1200, 60, "orig");
    let old = lines(30, 60, "old");
    let new = lines(32, 60, "new");
    let patch: Vec<String> = old
        .lines()
        .map(|l| format!("-{l}"))
        .chain(new.lines().map(|l| format!("+{l}")))
        .collect();
    let stdout = lines(150, 60, "out");
    let page = lines(1500, 40, "page");
    let call = |id: &str, name: &str, input: serde_json::Value| {
        record(
            &json!({"type": "assistant", "uuid": format!("a-{id}"), "message": {"id": format!("m-{id}"),
            "role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": input}]}}),
        )
    };
    let result = |id: &str, content: &str, structured: serde_json::Value| {
        record(
            &json!({"type": "user", "uuid": format!("r-{id}"), "message": {"role": "user",
            "content": [{"type": "tool_result", "tool_use_id": id, "content": content}]}, "toolUseResult": structured}),
        )
    };
    vec![
        record(&json!({"type": "user", "uuid": format!("u-{n}"),
            "message": {"role": "user", "content": format!("Change file {n}")}})),
        call(&format!("read-{n}"), "Read", json!({"file_path": file})),
        result(
            &format!("read-{n}"),
            &read,
            json!({"type": "text", "file": {"filePath": file, "content": read, "numLines": 800}}),
        ),
        call(
            &format!("edit-{n}"),
            "Edit",
            json!({"file_path": file, "old_string": old, "new_string": new, "replace_all": false}),
        ),
        result(
            &format!("edit-{n}"),
            "The file has been updated.",
            json!({"filePath": file, "oldString": old, "newString": new, "originalFile": original,
                "structuredPatch": [{"oldStart": 10, "oldLines": 30, "newStart": 10, "newLines": 32, "lines": patch}],
                "userModified": false, "replaceAll": false}),
        ),
        call(
            &format!("bash-{n}"),
            "Bash",
            json!({"command": "cargo test", "description": "Run the tests"}),
        ),
        result(
            &format!("bash-{n}"),
            &stdout,
            json!({"stdout": stdout, "stderr": "", "interrupted": false}),
        ),
        call(&format!("page-{n}"), "mcp__browser__snapshot", json!({})),
        result(&format!("page-{n}"), &page, json!([{"type": "text", "text": page}])),
        record(
            &json!({"type": "assistant", "uuid": format!("t-{n}"), "message": {"id": format!("m-t-{n}"),
            "role": "assistant", "content": [{"type": "text", "text": lines(20, 80, "answer")}]}}),
        ),
    ]
}

#[test]
#[ignore = "a memory budget (E25), run by make perf"]
fn a_long_session_is_shown_again_within_its_memory_budget() {
    let folder = scratch("memory");
    let path = folder.join("long.jsonl");
    let transcript: Vec<String> = (0..TURNS).flat_map(turn).collect();
    std::fs::write(&path, transcript.join("\n")).expect("the transcript is written");
    let size = std::fs::metadata(&path).expect("the transcript exists").len();
    drop(transcript);

    let before = HELD.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let thread = term_claude::history::rebuild(Thread::new(ThreadId::from("t"), "claude", &folder), &path)
        .expect("the transcript is read");
    let peak = PEAK.load(Ordering::Relaxed) - before;
    let kept = HELD.load(Ordering::Relaxed) - before;
    let mb = |bytes: usize| bytes as f64 / 1e6;
    println!(
        "transcript {:.1} MB: peak {:.1} MB, kept {:.1} MB",
        size as f64 / 1e6,
        mb(peak),
        mb(kept)
    );
    term_base::perf::value("term-claude: peak memory reading a long session", "MB", true, mb(peak));
    term_base::perf::value("term-claude: memory kept by a long session", "MB", true, mb(kept));
    assert_eq!(
        thread.entries(&Lane::Main).len(),
        TURNS * 6,
        "every message and call is on the timeline"
    );
    assert!(peak <= PEAK_BUDGET, "reading took {:.1} MB at its peak", mb(peak));
    assert!(kept <= KEPT_BUDGET, "the timeline keeps {:.1} MB", mb(kept));
}

/// The most reading the transcript may hold at once, its lines one at a time; whole it took 42 MB.
const PEAK_BUDGET: usize = 12_000_000;
/// The most the finished timeline may keep, the tool outputs cut to what a card shows; whole it kept 17 MB.
const KEPT_BUDGET: usize = 10_000_000;
