//! Protocol-level regressions: child streams cannot overwrite the main conversation, and only the server's
//! terminal list makes a shell command a background task.

use std::time::{Duration, UNIX_EPOCH};

use serde_json::{Value, json};
use term_thread::{EntryKind, Lane, Status, TaskStatus, Thread, ThreadId, ToolView};

use super::agent_key;
use crate::{Capabilities, Mapper};

struct Run {
    mapper: Mapper,
    thread: Thread,
    caps: Capabilities,
}

impl Run {
    fn new() -> Self {
        let mut mapper = Mapper::default();
        mapper.set_thread("main");
        Self {
            mapper,
            thread: Thread::new(ThreadId::from("t"), "codex", "/w"),
            caps: Capabilities::default(),
        }
    }

    fn note(&mut self, method: &str, params: Value) {
        let (updates, _) =
            self.mapper
                .notification(method, &params, &mut self.caps, UNIX_EPOCH + Duration::from_secs(10));
        self.thread.apply_all(updates);
    }

    fn start(&mut self, id: &str, parent: &str) {
        self.note(
            "thread/started",
            json!({"thread": {"id": id, "parentThreadId": parent, "agentRole": "reviewer"}}),
        );
        self.note("turn/started", json!({"threadId": id, "turn": {"id": "turn"}}));
    }
}

#[test]
fn child_and_nested_streams_keep_their_own_status_usage_and_item_namespace() {
    let mut run = Run::new();
    run.note("turn/started", json!({"threadId": "main", "turn": {"id": "turn"}}));
    run.note(
        "item/agentMessage/delta",
        json!({"threadId": "main", "turnId": "turn", "itemId": "text", "delta": "Parent"}),
    );
    run.start("child", "main");
    run.start("nested", "child");
    run.start("foreign", "someone_else");
    for (id, message) in [("child", "Child"), ("nested", "Nested"), ("foreign", "Hidden")] {
        run.note(
            "item/agentMessage/delta",
            json!({"threadId": id, "turnId": "turn", "itemId": "text", "delta": message}),
        );
        run.note(
            "item/completed",
            json!({"threadId": id, "turnId": "turn", "item": {"type": "agentMessage", "id": "text", "text": message}}),
        );
        run.note(
            "thread/tokenUsage/updated",
            json!({"threadId": id, "tokenUsage": {"total": {"totalTokens": 42}, "last": {"totalTokens": 7}}}),
        );
        run.note(
            "turn/completed",
            json!({"threadId": id, "turn": {"id": "turn", "status": "completed"}}),
        );
    }
    assert_eq!(run.thread.status(), Status::Running);
    assert_eq!(run.thread.subagents().len(), 2);
    let child = run.thread.subagent(&agent_key("child")).unwrap();
    assert_eq!(
        (child.status, child.tokens, child.summary.as_deref()),
        (TaskStatus::Completed, 42, Some("Child"))
    );
    assert_eq!(
        run.thread.subagent(&agent_key("nested")).unwrap().parent.as_deref(),
        Some(agent_key("child").as_str())
    );
    for (lane, message) in [
        (Lane::Main, "Parent"),
        (Lane::Subagent(agent_key("child")), "Child"),
        (Lane::Subagent(agent_key("nested")), "Nested"),
    ] {
        assert!(
            run.thread
                .entries(&lane)
                .iter()
                .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == message))
        );
    }
    assert!(
        run.thread
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::ToolCall(c) if matches!(c.view, ToolView::Agent { .. })))
    );
    assert!(run.mapper.task_stop(&agent_key("child")).is_none());
}

#[test]
fn spawn_status_and_wait_results_update_one_agent_and_child_approvals_are_nested() {
    let mut run = Run::new();
    run.start("child", "main");
    run.note("item/completed", json!({"threadId": "main", "turnId": "p", "item": {
        "type": "collabAgentToolCall", "id": "spawn", "tool": "spawnAgent", "status": "completed",
        "prompt": "Review the change", "receiverThreadIds": ["child"], "agentsStates": {"child": {"status": "running"}}
    }}));
    assert_eq!(run.thread.subagents().len(), 1);
    assert_eq!(run.thread.subagents()[0].description, "Review the change");
    let (offer, updates) = run
        .mapper
        .task_request(
            "a",
            crate::protocol::method::COMMAND_APPROVAL,
            &json!({"threadId": "child", "turnId": "turn", "itemId": "cmd", "command": "ls"}),
            UNIX_EPOCH,
        )
        .unwrap();
    run.thread.apply_all(updates);
    assert!(offer.key.starts_with("child:child:"));
    assert!(
        run.thread
            .entries(&Lane::Subagent(agent_key("child")))
            .iter()
            .any(|e| matches!(e.kind, EntryKind::PermissionRequest(_)))
    );
    assert!(
        run.mapper
            .task_request(
                "b",
                crate::protocol::method::COMMAND_APPROVAL,
                &json!({"threadId": "foreign"}),
                UNIX_EPOCH
            )
            .is_none()
    );
    run.note(
        "item/completed",
        json!({"threadId": "main", "turnId": "p", "item": {
            "type": "collabAgentToolCall", "id": "wait", "tool": "wait", "status": "completed",
            "receiverThreadIds": ["child"], "agentsStates": {"child": {"status": "completed", "message": "Done"}}
        }}),
    );
    assert_eq!(run.thread.subagents().len(), 1);
    assert_eq!(run.thread.subagents()[0].summary.as_deref(), Some("Done"));
}

#[test]
fn terminals_stream_output_survive_the_parent_turn_and_end_on_exit_or_session_end() {
    let mut run = Run::new();
    run.note(
        "item/started",
        json!({"threadId": "main", "turnId": "t", "item": {
            "type": "commandExecution", "id": "cmd", "command": "sleep 30", "status": "inProgress", "processId": "123"
        }}),
    );
    assert!(run.thread.background().is_empty());
    let updates = run.mapper.background_terminals(
        "main",
        &[json!({"itemId": "cmd", "processId": "123", "command": "sleep 30"})],
        UNIX_EPOCH,
    );
    run.thread.apply_all(updates);
    let key = run.thread.background()[0].id.clone();
    assert!(run.mapper.task_stop(&key).is_some());
    run.note(
        "item/commandExecution/outputDelta",
        json!({"threadId": "main", "turnId": "t", "itemId": "cmd", "delta": "tick\n"}),
    );
    run.note(
        "turn/completed",
        json!({"threadId": "main", "turn": {"id": "t", "status": "completed"}}),
    );
    assert_eq!(run.thread.background()[0].status, TaskStatus::Running);
    assert_eq!(run.thread.background()[0].output.text.as_deref(), Some("tick\n"));
    run.note(
        "item/completed",
        json!({"threadId": "main", "turnId": "t", "item": {
            "type": "commandExecution", "id": "cmd", "command": "sleep 30", "status": "completed", "exitCode": 2
        }}),
    );
    assert_eq!(run.thread.background()[0].status, TaskStatus::Failed);
    assert!(run.mapper.task_stop(&key).is_none());
    run.start("child", "main");
    let updates = run.mapper.end_tasks(UNIX_EPOCH + Duration::from_secs(20));
    run.thread.apply_all(updates);
    assert_eq!(run.thread.subagents()[0].status, TaskStatus::Stopped);
}

#[test]
fn early_child_results_are_replayed_only_after_ancestry_is_known_and_stay_final() {
    let mut run = Run::new();
    run.note(
        "turn/started",
        json!({"threadId": "child", "turn": {"id": "child-turn"}}),
    );
    run.note(
        "item/completed",
        json!({"threadId": "child", "turnId": "child-turn", "item": {
            "type": "agentMessage", "id": "result", "text": "Already done"
        }}),
    );
    run.note(
        "turn/completed",
        json!({"threadId": "child", "turn": {"id": "child-turn", "status": "completed"}}),
    );
    assert!(run.thread.subagents().is_empty());
    run.note(
        "item/completed",
        json!({"threadId": "main", "turnId": "parent-turn", "item": {
            "type": "collabAgentToolCall", "id": "spawn", "tool": "spawnAgent", "status": "completed",
            "prompt": "Fast job", "receiverThreadIds": ["child"], "agentsStates": {"child": {"status": "running"}}
        }}),
    );
    let sub = &run.thread.subagents()[0];
    assert_eq!(sub.status, TaskStatus::Completed);
    assert_eq!(sub.summary.as_deref(), Some("Already done"));
    assert_eq!(run.thread.entries(&Lane::Subagent(sub.key.clone())).len(), 1);
    assert!(
        run.thread
            .entries(&Lane::Main)
            .iter()
            .all(|e| !matches!(e.kind, EntryKind::AgentMessage(_)))
    );
}
