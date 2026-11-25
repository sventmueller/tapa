//! Each ending of a turn on each path, from the recordings of the session crates and from what each path's mapper
//! makes of a turn's end (C60).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use term_thread::{EntryKind, ErrorKind, Lane, Notice, Status, Thread, ThreadId, Update, UserMessage};

use super::{Outcome, last_entry, outcome};

fn fixture(krate: &str, name: &str) -> String {
    let path = format!("{}/../{krate}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn at() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn thread(agent: &str) -> Thread {
    Thread::new(ThreadId::from("fixture"), agent, "/w")
}

/// A user's prompt that opens a turn, as each path puts one before it sends.
fn prompt(thread: &mut Thread, key: &str) {
    thread.apply_all([
        Update::Put {
            lane: Lane::Main,
            key: Some(key.into()),
            at: at(),
            entry: EntryKind::UserMessage(UserMessage::default()),
        },
        Update::Status(Status::Running),
    ]);
}

#[test]
fn claude_ends_a_turn_as_its_result_frame_says() {
    for name in ["api_retry.ndjson", "interrupt.ndjson", "recorded_signed_out.ndjson"] {
        let mut replay = term_claude::replay::Replay::new(thread("claude"), at(), Duration::from_millis(10));
        replay.feed_all(&fixture("term-claude", name));
        let thread = replay.into_thread();
        assert_eq!(thread.status(), Status::Idle, "{name}");
        let ended = outcome(&thread, None);
        match name {
            "api_retry.ndjson" => assert_eq!(ended, Outcome::Succeeded, "a retried request that then answered"),
            "interrupt.ndjson" => assert_eq!(ended, Outcome::Cancelled),
            _ => assert!(
                matches!(&ended, Outcome::Failed(m) if m.contains("/login")),
                "{ended:?}"
            ),
        }
    }
}

/// Plays the Codex notifications `recording`, the outcome of each turn as `turn/completed` ends it.
fn codex_turns(recording: &str) -> Vec<Outcome> {
    let mut mapper = term_codex::Mapper::new(0);
    let mut caps = term_codex::Capabilities::default();
    let mut thread = thread(term_codex::AGENT);
    let mut since = None;
    let mut ended = Vec::new();
    for line in recording.lines().filter(|l| !l.trim().is_empty()) {
        let frame: Value = serde_json::from_str(line).unwrap();
        let method = frame["method"].as_str().unwrap_or_default();
        if method == "turn/started" {
            since = last_entry(&thread);
        }
        let (updates, _) = mapper.notification(method, &frame["params"], &mut caps, at());
        thread.apply_all(updates);
        if method == "turn/completed" {
            assert_eq!(
                thread.status(),
                Status::Idle,
                "Codex rests after every turn, failed ones too"
            );
            ended.push(outcome(&thread, since));
        }
    }
    ended
}

#[test]
fn codex_ends_turns_as_completed_interrupted_or_failed() {
    assert_eq!(
        codex_turns(&fixture("term-codex", "tools.ndjson")),
        [Outcome::Succeeded]
    );
    let endings = codex_turns(&fixture("term-codex", "endings.ndjson"));
    let [cancelled, expired, full] = endings.as_slice() else {
        panic!("{endings:?}")
    };
    assert_eq!(*cancelled, Outcome::Cancelled);
    assert!(
        matches!(expired, Outcome::Failed(m) if m.starts_with("Your session has expired.")),
        "{expired:?}"
    );
    assert_eq!(
        *full,
        Outcome::Failed("Context window exceeded.".into()),
        "one error, not two"
    );
}

/// Plays the OpenCode events `recording`, the outcome of each turn as it rests again.
fn opencode_turns(recording: &str) -> Vec<Outcome> {
    let mut mapper = term_opencode::Mapper::new(0);
    let mut caps = term_opencode::Capabilities::default();
    let mut thread = thread(term_opencode::AGENT);
    let mut since = None;
    let mut ended = Vec::new();
    for line in recording.lines().filter(|l| !l.trim().is_empty()) {
        let event: Value = serde_json::from_str(line).unwrap();
        let kind = event["type"].as_str().unwrap_or_default().to_string();
        if kind == "session.execution.started" {
            since = last_entry(&thread);
        }
        let was = thread.status();
        let (updates, _) = mapper.event(&event, &mut caps, at());
        thread.apply_all(updates);
        if was != Status::Idle && thread.status() == Status::Idle {
            ended.push(outcome(&thread, since));
        }
    }
    ended
}

#[test]
fn opencode_ends_turns_as_succeeded_interrupted_or_failed_and_a_shutdown_loses_the_connection() {
    let endings = opencode_turns(&fixture("term-opencode", "endings.ndjson"));
    let [cancelled, key, broke, shutdown] = endings.as_slice() else {
        panic!("{endings:?}")
    };
    assert_eq!(*cancelled, Outcome::Cancelled);
    assert!(
        matches!(key, Outcome::Failed(m) if m.starts_with("Invalid API key")),
        "{key:?}"
    );
    assert_eq!(*broke, Outcome::Failed("Something broke".into()));
    assert_eq!(*shutdown, Outcome::ConnectionLost);
    let tools = opencode_turns(&fixture("term-opencode", "tools.ndjson"));
    assert!(
        tools.iter().all(|o| *o == Outcome::Succeeded) && !tools.is_empty(),
        "{tools:?}"
    );
}

/// The thread of an ACP turn: a prompt, a chunk of the answer, then the agent's answer to `session/prompt`.
fn acp_turn(answer: Value) -> Outcome {
    let mut mapper = term_acp::mapping::Mapper::new();
    let mut caps = term_acp::Capabilities::default();
    let mut thread = thread("gemini");
    prompt(&mut thread, "u1");
    let chunk = json!({"sessionId": "s-1", "update": {"sessionUpdate": "agent_message_chunk",
                       "content": {"type": "text", "text": "Half"}}});
    let (updates, _) = mapper.apply_json(&chunk, &mut caps, at());
    thread.apply_all(updates);
    thread.apply_all(mapper.prompt_answered_json(&answer, at()));
    assert_eq!(thread.status(), Status::Idle);
    outcome(&thread, None)
}

#[test]
fn acp_ends_a_turn_by_its_stop_reason() {
    assert_eq!(acp_turn(json!({"stopReason": "end_turn"})), Outcome::Succeeded);
    assert_eq!(acp_turn(json!({"stopReason": "cancelled"})), Outcome::Cancelled);
    assert_eq!(
        acp_turn(json!({"stopReason": "max_tokens"})),
        Outcome::Failed("max_tokens".into())
    );
    assert_eq!(
        acp_turn(json!({"stopReason": "refusal"})),
        Outcome::Failed("refusal".into())
    );
}

#[test]
fn a_process_that_ends_mid_turn_leaves_an_error_of_the_process() {
    let mut thread = thread("gemini");
    prompt(&mut thread, "u1");
    thread.apply_all([
        Update::Put {
            lane: Lane::Main,
            key: Some("n1".into()),
            at: at(),
            entry: EntryKind::Notice(Notice::Error {
                kind: ErrorKind::Process,
                message: "the agent exited".into(),
            }),
        },
        Update::Status(Status::Idle),
    ]);
    assert_eq!(outcome(&thread, None), Outcome::ConnectionLost);
}
