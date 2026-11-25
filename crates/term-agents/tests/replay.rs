//! A played recording of Claude as the driver's backend: what it plays reaches the subscribers at once, and it
//! takes no prompts.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use std::time::{Duration, SystemTime};

use support::{Harness, folder};
use term_agents::{AgentPath, Launch, SendError, UserChoices, UserRule};
use term_claude::replay::Replay;
use term_thread::{Lane, Thread, ThreadId, UserMessage};

const RECORDING: &str = include_str!("../../term-claude/tests/fixtures/plan_todos.ndjson");

#[test]
fn a_played_recording_reaches_its_subscribers_at_once_and_takes_no_prompts() {
    let scratch = folder("replay");
    let thread = Thread::new(ThreadId::from("t"), term_claude::AGENT, scratch.to_path_buf());
    let replay = Replay::new(thread, SystemTime::now(), Duration::from_millis(10));
    let launch = Launch::new(
        AgentPath::Replay(Box::new(replay)),
        scratch.to_path_buf(),
        Vec::new(),
        UserChoices::default(),
    );
    let mut harness = Harness::new();
    let session = harness.registry.start(launch, Box::new(UserRule)).unwrap();
    let driver = harness.registry.get_mut(session).unwrap();
    let view = driver.subscribe();
    assert!(driver.take(view).unwrap().is_redraw(), "it starts with the snapshot");
    let changes = driver.feed(RECORDING);
    assert!(!changes.is_empty());
    let batch = driver.take(view).expect("the view's batch");
    assert!(!batch.changes.is_empty(), "without a hop to the main thread");
    let played = batch.thread.entries(&Lane::Main).len();
    assert!(played > 0);
    assert_eq!(driver.thread().entries(&Lane::Main).len(), played);
    assert!(driver.is_replay() && !driver.takes_prompts());
    let message = UserMessage {
        text: "Again".into(),
        ..UserMessage::default()
    };
    assert!(matches!(driver.steer(message), Err(SendError::Refused(_))));
    assert!(driver.claude().is_none() && driver.claude_capabilities().is_none());
}
