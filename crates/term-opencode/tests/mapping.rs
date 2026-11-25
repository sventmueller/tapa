//! Event sequences cut from the 2.2.6 schema's event types, checked against it and mapped onto a thread: every tool
//! the cards know, the streaming of text and reasoning, usage, compaction, retries, the ends of turns and what is
//! left out.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use std::time::{Duration, UNIX_EPOCH};

use serde_json::Value;
use term_opencode::{Capabilities, Mapper};
use term_thread::{
    EntryKind, ErrorKind, InfoLevel, Lane, Notice, Status, Thread, ThreadId, TodoStatus, ToolKind, ToolOutput,
    ToolStatus, ToolView,
};

fn events(name: &str) -> Vec<Value> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .enumerate()
        .map(|(n, line)| serde_json::from_str(line).unwrap_or_else(|e| panic!("{name}:{}: {e}", n + 1)))
        .collect()
}

/// Plays the fixture `name` and returns the thread, the capabilities and whether any event changed them.
fn play(name: &str) -> (Thread, Capabilities, bool) {
    let mut mapper = Mapper::new(7);
    mapper.set_session("ses_1");
    let mut caps = Capabilities {
        models: vec![term_opencode::Model {
            id: "claude-sonnet-4".into(),
            model_id: "claude-sonnet-4".into(),
            provider: "anthropic".into(),
            name: "Claude Sonnet 4".into(),
            variants: vec!["high".into()],
            input: vec!["text".into(), "image".into()],
            context: Some(200_000),
            status: "active".into(),
            enabled: true,
        }],
        model: term_opencode::ModelRef::parse("anthropic/claude-sonnet-4"),
        ..Capabilities::default()
    };
    let mut thread = Thread::new(ThreadId::from("t"), term_opencode::AGENT, "/w");
    let mut learned = false;
    for event in events(name) {
        let (updates, changed) = mapper.event(&event, &mut caps, UNIX_EPOCH);
        learned |= changed;
        thread.apply_all(updates);
    }
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
fn the_fixtures_are_events_of_the_schema() {
    let spec = support::schema::Spec::load();
    for name in ["tools.ndjson", "endings.ndjson"] {
        for (n, event) in events(name).iter().enumerate() {
            if let Err(error) = spec.check_event(event) {
                panic!("{name}:{}: {error}", n + 1);
            }
        }
    }
    let wrong = serde_json::json!({"id": "evt_1", "created": 1, "type": "session.text.delta",
        "data": {"sessionID": "ses_1", "assistantMessageID": "msg_1", "ordinal": 0}});
    assert!(
        spec.check_event(&wrong).is_err(),
        "a delta without its delta is not an event"
    );
}

#[test]
fn a_turn_with_every_tool_maps_onto_cards_messages_and_notices() {
    let (thread, caps, learned) = play("tools.ndjson");
    let kinds = kinds(&thread);
    let EntryKind::Thought(thought) = &kinds[0] else {
        panic!("{kinds:?}")
    };
    assert_eq!(thought.text, "First. Second.");
    assert!(!thought.streaming);
    assert_eq!(thought.duration, Some(Duration::from_secs(3)));
    assert!(matches!(&kinds[1], EntryKind::AgentMessage(m) if m.markdown == "Searching the docs." && !m.streaming));
    assert!(
        !kinds
            .iter()
            .any(|k| matches!(k, EntryKind::AgentMessage(m) if m.markdown.contains("not ours"))),
        "another session's events are left out"
    );

    let glob = tool(&kinds, "c_glob");
    assert_eq!(
        (glob.name.as_str(), glob.kind, glob.status),
        ("Glob", ToolKind::Search, ToolStatus::Completed)
    );
    assert!(
        matches!(&glob.view, ToolView::Search { glob: Some(g), path: Some(p), .. } if g == "**/*.rs" && p == "src")
    );
    assert_eq!(glob.output, Some(ToolOutput::Text("src/a.rs".into())));
    assert!(matches!(&tool(&kinds, "c_grep").view, ToolView::Search { pattern, .. } if pattern == "fn main"));
    let fetch = tool(&kinds, "c_fetch");
    assert_eq!((fetch.name.as_str(), fetch.kind), ("WebFetch", ToolKind::Fetch));
    assert!(matches!(&fetch.view, ToolView::Fetch { url, .. } if url == "https://example.test/doc"));
    assert_eq!(
        tool(&kinds, "c_search").view,
        ToolView::WebSearch {
            query: "opencode api".into()
        }
    );
    let task = tool(&kinds, "c_task");
    assert_eq!(task.name, "Task");
    assert!(
        matches!(&task.view, ToolView::Agent { description, agent_type: Some(a), .. }
        if description == "Find usages" && a == "general")
    );
    let write = tool(&kinds, "c_write");
    assert_eq!(write.name, "Write");
    assert!(
        matches!(&write.output, Some(ToolOutput::Diff { path, hunks }) if path == "new.txt" && hunks[0].lines == ["+hello", "+world"])
    );
    let patch = tool(&kinds, "c_patch");
    assert_eq!((patch.name.as_str(), patch.title.as_str()), ("Edit", "a.rs"));
    assert!(matches!(&patch.output, Some(ToolOutput::Diff { hunks, .. }) if hunks[0].lines == ["-a", "+b"]));
    let mcp = tool(&kinds, "c_mcp");
    assert_eq!((mcp.name.as_str(), mcp.status), ("docs_lookup", ToolStatus::Failed));
    assert_eq!(mcp.output, Some(ToolOutput::Text("offline".into())));
    let shell = tool(&kinds, "c_shell");
    assert_eq!((shell.name.as_str(), shell.kind), ("Bash", ToolKind::Execute));
    assert!(
        matches!(&shell.view, ToolView::Command { command, timeout: Some(t), .. }
        if command == "cargo test" && *t == Duration::from_secs(60))
    );
    assert!(matches!(&shell.output, Some(ToolOutput::Terminal { stdout, .. }) if stdout == "running 3 tests\nok"));
    assert!(shell.elapsed.is_some());

    let todos = thread.todos();
    let states: Vec<TodoStatus> = todos.iter().map(|t| t.status).collect();
    assert_eq!(
        states,
        [TodoStatus::Completed, TodoStatus::InProgress, TodoStatus::Completed]
    );
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EntryKind::Todos(list) if list.len() == 3))
    );
    assert!(kinds.iter().any(
        |k| matches!(k, EntryKind::Notice(Notice::Info { level: InfoLevel::Warning, text })
        if text.starts_with("Overloaded"))
    ));
    assert!(kinds.iter().any(
        |k| matches!(k, EntryKind::Notice(Notice::Compaction { manual: false, tokens_before, .. })
        if *tokens_before == 12_850)
    ));
    assert!(matches!(kinds.last(), Some(EntryKind::AgentMessage(m)) if m.markdown == "All done." && !m.streaming));

    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.title(), "Docs search");
    assert_eq!(thread.mode(), Some("plan"));
    assert_eq!(thread.model(), Some("openai/gpt-5"));
    assert_eq!(thread.effort(), Some("high"));
    assert!(learned);
    assert_eq!(caps.mode(), Some("plan"));
    let usage = thread.usage();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.cache_read_tokens),
        (2600, 400, 22000)
    );
    assert_eq!(usage.cost_usd, Some(0.031));
    assert_eq!((usage.context_used, usage.context_size), (Some(12_540), Some(200_000)));
    assert_eq!(usage.turns.len(), 1);
    let turn = &usage.turns[0];
    assert_eq!(turn.tokens.unwrap().input, 2500);
    assert!((turn.cost_usd.unwrap() - 0.03).abs() < 1e-9);
    assert_eq!(turn.duration, Some(Duration::from_secs(73)));
}

#[test]
fn interrupted_and_failed_turns_end_with_one_notice_each() {
    let (thread, _, _) = play("endings.ndjson");
    let kinds = kinds(&thread);
    assert!(matches!(&kinds[0], EntryKind::AgentMessage(m) if m.markdown == "Half an" && !m.streaming));
    let run = tool(&kinds, "c_run");
    assert_eq!((run.name.as_str(), run.status), ("Bash", ToolStatus::Failed));
    assert!(matches!(kinds[2], EntryKind::Notice(Notice::Interrupted)));
    let errors: Vec<(ErrorKind, String)> = kinds
        .iter()
        .filter_map(|k| match k {
            EntryKind::Notice(Notice::Error { kind, message }) => Some((*kind, message.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert_eq!(errors[0].0, ErrorKind::Auth);
    assert_eq!(errors[0].1, "Invalid API key\n{\"error\":\"bad key\"}");
    assert_eq!(errors[1], (ErrorKind::Api, "Something broke".to_string()));
    assert_eq!(errors[2], (ErrorKind::Compaction, "Too long".to_string()));
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EntryKind::Notice(Notice::Info { text, .. }) if text == "interrupted: shutdown"))
    );
    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.activity(), None);
    assert_eq!(thread.usage().turns.len(), 4);
}
