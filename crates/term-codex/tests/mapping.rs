//! Notification sequences, written by hand after the 0.167 schema, mapped onto a thread: every item type the mapper
//! reads, the streaming of text and reasoning, the ends of turns and what is left out.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;
use term_codex::{Account, Capabilities, Mapper};
use term_thread::{
    Activity, EntryKind, ErrorKind, InfoLevel, Lane, Notice, Status, Thread, ThreadId, ToolKind, ToolOutput,
    ToolStatus, ToolView,
};

/// Plays the fixture `name` and returns the thread, the capabilities and whether any notification changed them.
fn play(name: &str) -> (Thread, Capabilities, bool) {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(path).unwrap();
    let mut mapper = Mapper::new(7);
    mapper.set_thread("thr_1");
    let mut caps = Capabilities {
        model: Some("gpt-5.5".into()),
        ..Capabilities::default()
    };
    let mut thread = Thread::new(ThreadId::from("t"), term_codex::AGENT, "/w");
    let mut learned = false;
    let mut compacting = false;
    for (n, line) in text.lines().enumerate() {
        let message: Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("line {}: {e}", n + 1));
        let method = message["method"].as_str().unwrap();
        let at = UNIX_EPOCH + Duration::from_secs(n as u64);
        let (updates, changed) = mapper.notification(method, &message["params"], &mut caps, at);
        learned |= changed;
        thread.apply_all(updates);
        compacting |= thread.activity() == Some(Activity::Compacting);
    }
    assert!(
        name != "tools.ndjson" || compacting,
        "compaction shows as activity while it runs"
    );
    (thread, caps, learned)
}

fn kinds(thread: &Thread) -> Vec<EntryKind> {
    thread.entries(&Lane::Main).iter().map(|e| e.kind.clone()).collect()
}

fn tool(kinds: &[EntryKind], id: &str) -> term_thread::ToolCall {
    kinds
        .iter()
        .find_map(|k| match k {
            EntryKind::ToolCall(c) if c.call_id == id => Some(c.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool {id} in {kinds:?}"))
}

#[test]
fn a_turn_with_every_tool_maps_onto_cards_messages_and_notices() {
    let (thread, caps, learned) = play("tools.ndjson");
    let kinds = kinds(&thread);
    assert!(matches!(&kinds[0], EntryKind::UserMessage(m) if m.text == "Search the docs"));
    let EntryKind::Thought(thought) = &kinds[1] else {
        panic!("{kinds:?}")
    };
    assert_eq!(thought.text, "First.\n\nSecond.");
    assert!(!thought.streaming);
    assert_eq!(thought.duration, Some(Duration::from_secs(2)));

    let search = tool(&kinds, "w1");
    assert_eq!((search.kind, search.status), (ToolKind::Fetch, ToolStatus::Completed));
    assert_eq!(
        search.view,
        ToolView::WebSearch {
            query: "codex app-server".into()
        }
    );
    let page = tool(&kinds, "w2");
    assert!(matches!(&page.view, ToolView::Fetch { url, .. } if url == "https://example.test/doc"));
    assert_eq!(page.status, ToolStatus::Completed);

    let mcp = tool(&kinds, "m1");
    assert_eq!(
        mcp.view,
        ToolView::Mcp {
            server: "docs".into(),
            tool: "search".into()
        }
    );
    assert_eq!(mcp.output, Some(ToolOutput::Text("newline-delimited".into())));
    assert_eq!(mcp.elapsed, Some(Duration::from_millis(100)));

    let file = tool(&kinds, "f1");
    assert!(
        matches!(&file.view, ToolView::Write { content, .. } if content.is_empty()),
        "once the diff is there, it stands for the proposed content"
    );
    let Some(ToolOutput::Diff { hunks, .. }) = &file.output else {
        panic!("{file:?}")
    };
    assert_eq!(hunks[0].lines, ["+# Notes", "+line"]);

    let command = tool(&kinds, "c1");
    assert_eq!((command.kind, command.status), (ToolKind::Search, ToolStatus::Failed));
    assert!(matches!(&command.view, ToolView::Search { pattern, .. } if pattern == "framing"));
    assert!(matches!(&command.output, Some(ToolOutput::Terminal { stdout, .. }) if stdout == "docs/codex.md:3"));

    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EntryKind::Notice(Notice::Compaction { manual: false, .. })))
    );
    assert!(kinds.iter().any(
        |k| matches!(k, EntryKind::Notice(Notice::Info { level: InfoLevel::Warning, text })
        if text == "Reconnecting... 1/5")
    ));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EntryKind::Notice(Notice::Info { text, .. })
        if text == "gpt-5.5 → gpt-5.5-mini"))
    );
    assert!(matches!(kinds.last(), Some(EntryKind::AgentMessage(m)) if m.markdown == "Found it." && !m.streaming));
    assert!(
        !kinds
            .iter()
            .any(|k| matches!(k, EntryKind::AgentMessage(m) if m.markdown.contains("subagent"))),
        "notifications of other threads are left out"
    );
    assert_eq!(kinds.len(), 11, "{kinds:#?}");

    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.activity(), None);
    assert_eq!(thread.model(), Some("gpt-5.5-mini"));
    assert_eq!(caps.model.as_deref(), Some("gpt-5.5-mini"));
    assert!(learned);
    let turn = thread.usage().last_turn().unwrap();
    assert_eq!(turn.duration, Some(Duration::from_millis(3500)));
    assert_eq!(
        turn.tokens, None,
        "without token reports the turn's tokens stay unknown"
    );
}

#[test]
fn interrupted_and_failed_turns_end_with_one_notice_each() {
    let (thread, caps, learned) = play("endings.ndjson");
    let kinds = kinds(&thread);
    assert!(matches!(&kinds[0], EntryKind::AgentMessage(m) if m.markdown == "Half an" && !m.streaming));
    assert!(matches!(&kinds[1], EntryKind::Notice(Notice::Interrupted)));
    assert!(
        matches!(&kinds[2], EntryKind::Notice(Notice::Error { kind: ErrorKind::Auth, message })
        if message == "Your session has expired.\nSign in again.")
    );
    assert!(
        matches!(&kinds[3], EntryKind::Notice(Notice::Error { kind: ErrorKind::Turn, message })
        if message == "Context window exceeded.")
    );
    assert!(
        matches!(&kinds[4], EntryKind::Notice(Notice::Info { level: InfoLevel::Warning, text })
        if text == "Unknown key foo\nin config.toml line 3")
    );
    assert_eq!(kinds.len(), 5, "{kinds:#?}");
    assert_eq!(thread.title(), "Expired session");
    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.usage().turns.len(), 3);
    assert_eq!(caps.account, Some(Account::ApiKey));
    assert!(!thread.usage().subscription);
    assert!(learned);
}

#[test]
fn every_line_of_the_fixtures_is_a_notification_the_schema_names() {
    let methods: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/schema/methods.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let known: Vec<&str> = methods["serverNotifications"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for name in ["tools.ndjson", "endings.ndjson"] {
        let text = std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for line in text.lines() {
            let message: Value = serde_json::from_str(line).unwrap();
            assert!(message.get("jsonrpc").is_none() && message.get("id").is_none());
            let method = message["method"].as_str().unwrap();
            assert!(known.contains(&method), "{name}: {method} is no notification of 0.167");
        }
    }
}

#[test]
fn the_initialize_parameters_name_only_what_the_schema_defines() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/schema/InitializeParams.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let params = term_codex::initialize_params();
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    let known = |definition: &Value, key: &str| definition["properties"].get(key).is_some();
    for key in keys(&params) {
        assert!(known(&schema, &key), "initialize: {key}");
    }
    let client = &schema["definitions"]["ClientInfo"];
    for key in keys(&params["clientInfo"]) {
        assert!(known(client, &key), "clientInfo: {key}");
    }
    for required in client["required"].as_array().unwrap() {
        assert!(
            params["clientInfo"][required.as_str().unwrap()].is_string(),
            "clientInfo needs {required}"
        );
    }
    let capabilities = &schema["definitions"]["InitializeCapabilities"];
    for key in keys(&params["capabilities"]) {
        assert!(known(capabilities, &key), "capabilities: {key}");
    }
    assert_eq!(params["capabilities"]["experimentalApi"], true);
}
