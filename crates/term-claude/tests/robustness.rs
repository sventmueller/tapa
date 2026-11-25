//! The session against stand-ins for `claude` that misbehave: one that never answers, one that stops reading what the
//! app writes, and one whose last frame lacks its newline or is cut off.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use term_base::testing::scratch;
use term_claude::{ControlError, Event, Launch, Session, SessionError};
use term_process::Supervisor;
use term_thread::{EntryKind, Lane, Thread, ThreadId, UserMessage};

const WAIT: Duration = Duration::from_secs(10);

/// How long a control request waits for its answer in these tests; `initialize` waits as long, so a busy machine must still answer it in time.
const SHORT: Duration = Duration::from_secs(2);

/// A stand-in in `dir` that keeps the first line it reads, prints `output` as it is, then runs `then`.
fn stand_in(dir: &Path, output: &str, then: &str) -> Launch {
    std::fs::write(dir.join("output"), output).unwrap();
    let script = dir.join("claude");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nread -r first\nprintf '%s\\n' \"$first\" > \"$OUT\"\ncat \"$OUTPUT\"\n{then}\n"),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("OUTPUT".to_string(), dir.join("output").to_string_lossy().into_owned()),
        (
            "OUT".to_string(),
            dir.join("written.ndjson").to_string_lossy().into_owned(),
        ),
    ];
    let mut launch = Launch::new(script, env, dir);
    launch.control_timeout = Some(SHORT);
    launch
}

/// The answer to `initialize`, as a line.
fn initialized() -> String {
    let answer = json!({"commands": [], "models": [], "account": {"subscriptionType": "pro", "apiProvider": "firstParty"}, "output_style": "default"});
    format!(
        "{}\n",
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-1", "response": answer}})
    )
}

fn assistant(text: &str) -> String {
    json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "text", "text": text}]}, "parent_tool_use_id": null})
        .to_string()
}

fn start(launch: &Launch, supervisor: &Supervisor) -> (Session, Receiver<Event>) {
    let (tx, rx) = channel();
    let thread = Thread::new(ThreadId::new(), "claude", launch.cwd.clone());
    let session = Session::start(supervisor, launch, thread, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (session, rx)
}

fn wait_for(rx: &Receiver<Event>, mut want: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = rx.recv_timeout(left).expect("the awaited event came in time");
        if want(&event) {
            return event;
        }
    }
}

/// Waits until the stand-in kept a frame `want` accepts.
fn wait_written(dir: &Path, want: impl Fn(&Value) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let text = std::fs::read_to_string(dir.join("written.ndjson")).unwrap_or_default();
        if text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .any(|f| want(&f))
        {
            return;
        }
        assert!(Instant::now() < deadline, "the stand-in never got the frame: {text}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn says(session: &Session, text: &str) -> bool {
    session.with_thread(|t| {
        t.entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == text))
    })
}

#[test]
fn an_initialize_that_is_never_answered_is_reported_and_withdrawn() {
    let dir = scratch("claude-hung-initialize");
    let launch = stand_in(&dir, "", "cat >> \"$OUT\"");
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    let failed = wait_for(&rx, |e| matches!(e, Event::Failed(_)));
    assert!(matches!(failed, Event::Failed(SessionError::Start(_))), "{failed:?}");
    wait_written(&dir, |f| {
        f["type"] == "control_cancel_request" && f["request_id"] == "req-1"
    });
    session.stop(Duration::from_millis(100));
}

#[test]
fn an_unanswered_control_request_times_out_for_a_waiter_and_a_callback() {
    let dir = scratch("claude-unanswered");
    let launch = stand_in(&dir, &initialized(), "cat >> \"$OUT\"");
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |e| matches!(e, Event::Capabilities));
    let asked = Instant::now();
    assert_eq!(session.interrupt().wait(), Err(ControlError::Timeout));
    assert!(asked.elapsed() < WAIT / 2, "waited {:?}", asked.elapsed());
    let (tx, answered) = channel();
    session.set_model(Some("haiku")).then(move |result| {
        let _ = tx.send(result);
    });
    assert_eq!(answered.recv_timeout(WAIT).unwrap(), Err(ControlError::Timeout));
    wait_written(&dir, |f| {
        f["type"] == "control_cancel_request" && f["request_id"] == "req-3"
    });
    assert!(!session.is_ended(), "a timeout does not end the session");
    session.stop(Duration::from_millis(100));
}

#[test]
fn a_cli_that_stops_reading_congests_and_its_requests_fail_at_once() {
    let dir = scratch("claude-congested");
    let launch = stand_in(&dir, &initialized(), "sleep 60");
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |e| matches!(e, Event::Capabilities));
    let message = UserMessage {
        text: "x".repeat(4096),
        ..UserMessage::default()
    };
    let mut sent = 0;
    let refused = loop {
        match session.send(message.clone()) {
            Ok(_) => sent += 1,
            Err(error) => break error,
        }
        assert!(sent < 10_000, "the queue never filled");
    };
    assert_eq!(refused, SessionError::Congested);
    let asked = Instant::now();
    assert_eq!(session.interrupt().wait(), Err(ControlError::Congested));
    assert!(asked.elapsed() < SHORT, "waited {:?}", asked.elapsed());
    session.stop(Duration::from_millis(100));
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

#[test]
fn the_last_frame_counts_without_its_newline_and_a_cut_off_one_is_dropped() {
    let dir = scratch("claude-last-line");
    let launch = stand_in(&dir, &format!("{}{}", initialized(), assistant("Bye")), "exit 0");
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert!(says(&session, "Bye"));

    let dir = scratch("claude-cut-off");
    let whole = assistant("Hello");
    let cut = &assistant("Lost")[..30];
    let launch = stand_in(&dir, &format!("{}{whole}\n{cut}", initialized()), "exit 0");
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert!(says(&session, "Hello"));
    assert!(!says(&session, "Lost"));
}
