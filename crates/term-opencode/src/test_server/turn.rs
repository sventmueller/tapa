//! The fixed turns the stand-in plays, as the events OpenCode 2.2.6 sends for them.

use std::collections::VecDeque;

use serde_json::{Value, json};

use super::{COMMAND_OUTPUT, FORM, PERMISSION, SESSION};

/// One thing a turn does.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Step {
    /// An event of this type with this `data`.
    Event(&'static str, Value),
    /// A permission request; the turn waits for its reply.
    Permission(Value),
    /// A form; the turn waits for its reply or cancellation.
    Form(Value),
    /// The turn waits to be interrupted.
    Hang,
    /// The server says `CRASH` on stderr and exits.
    Crash,
    /// Every event stream ends; the rest is played while none is open.
    Gap,
    /// The session's messages from now on, as a page's `data`.
    Record(Value),
}

fn event(kind: &'static str, mut data: Value) -> Step {
    data["sessionID"] = json!(SESSION);
    Step::Event(kind, data)
}

fn on(message: &str, mut data: Value) -> Value {
    data["assistantMessageID"] = json!(message);
    data
}

fn model() -> Value {
    json!({"providerID": "anthropic", "id": "claude-sonnet-4"})
}

fn step_started(message: &str) -> Step {
    event(
        "session.step.started",
        on(message, json!({"agent": "build", "model": model(), "started": 1})),
    )
}

fn step_ended(message: &str, finish: &str, cost: f64, tokens: Value) -> Step {
    event(
        "session.step.ended",
        on(message, json!({"finish": finish, "cost": cost, "tokens": tokens})),
    )
}

fn streamed(kind: &'static str, message: &str, ordinal: u64, parts: &[&str]) -> Vec<Step> {
    let started: &'static str = if kind == "text" {
        "session.text.started"
    } else {
        "session.reasoning.started"
    };
    let (delta, ended): (&'static str, &'static str) = if kind == "text" {
        ("session.text.delta", "session.text.ended")
    } else {
        ("session.reasoning.delta", "session.reasoning.ended")
    };
    let mut steps = vec![event(started, on(message, json!({"ordinal": ordinal})))];
    for part in parts {
        steps.push(event(delta, on(message, json!({"ordinal": ordinal, "delta": part}))));
    }
    steps.push(event(
        ended,
        on(message, json!({"ordinal": ordinal, "text": parts.concat()})),
    ));
    steps
}

fn tool_called(message: &str, id: &str, name: &str, input: Value) -> Vec<Step> {
    vec![
        event(
            "session.tool.input.started",
            on(message, json!({"id": id, "name": name})),
        ),
        event(
            "session.tool.input.delta",
            on(message, json!({"id": id, "delta": input.to_string()})),
        ),
        event(
            "session.tool.input.ended",
            on(message, json!({"id": id, "text": input.to_string()})),
        ),
        event(
            "session.tool.called",
            on(message, json!({"id": id, "input": input, "executed": false})),
        ),
    ]
}

fn tool_success(message: &str, id: &str, text: &str, metadata: Option<Value>) -> Step {
    let mut data = on(
        message,
        json!({"id": id, "content": [{"type": "text", "text": text}], "executed": false}),
    );
    if let Some(metadata) = metadata {
        data["metadata"] = metadata;
    }
    event("session.tool.success", data)
}

fn tool_failed(message: &str, id: &str, kind: &str, text: &str) -> Step {
    event(
        "session.tool.failed",
        on(
            message,
            json!({"id": id, "error": {"type": kind, "message": text}, "executed": false}),
        ),
    )
}

fn tokens(input: u64, output: u64, reasoning: u64, read: u64, write: u64) -> Value {
    json!({"input": input, "output": output, "reasoning": reasoning, "cache": {"read": read, "write": write}})
}

/// The turn for a prompt of `text`.
pub(super) fn turn(text: &str) -> VecDeque<Step> {
    let mut steps = vec![event("session.execution.started", json!({})), step_started("msg_a1")];
    if text.contains("crash") {
        steps.extend(streamed("text", "msg_a1", 0, &["Working"]));
        steps.push(Step::Crash);
    } else if text.contains("slow") {
        steps.push(event("session.text.started", on("msg_a1", json!({"ordinal": 0}))));
        steps.push(event(
            "session.text.delta",
            on("msg_a1", json!({"ordinal": 0, "delta": "Working"})),
        ));
        steps.push(Step::Hang);
    } else if text.contains("gap") {
        steps.extend(streamed("text", "msg_a1", 0, &["Working"]));
        steps.push(Step::Gap);
        steps.extend(gap_ending(text));
    } else if text.contains("ask") {
        steps.extend(question());
    } else if text.contains("fail") {
        let error = json!({"type": "provider.auth", "message": "Invalid API key", "status": 401});
        steps.push(event("session.step.failed", on("msg_a1", json!({"error": error}))));
        steps.push(event("session.execution.failed", json!({"error": error})));
    } else {
        steps.extend(opening());
    }
    steps.into()
}

fn opening() -> Vec<Step> {
    let mut steps = streamed("reasoning", "msg_a1", 0, &["Reading ", "the files."]);
    steps.push(Step::Event(
        "tui.toast.show",
        json!({"variant": "info", "message": "not for the session"}),
    ));
    steps.extend(streamed("text", "msg_a1", 1, &["Let me ", "look."]));
    let todos = json!({"todos": [{"content": "Read the file", "status": "in_progress"},
                                 {"content": "Fix it", "status": "pending"}]});
    steps.extend(tool_called("msg_a1", "call_todo", "todowrite", todos));
    steps.push(tool_success("msg_a1", "call_todo", "1/2 completed", None));
    steps.extend(tool_called("msg_a1", "call_read", "read", json!({"path": "a.txt"})));
    steps.push(tool_success("msg_a1", "call_read", "one\ntwo", None));
    steps.extend(tool_called(
        "msg_a1",
        "call_shell",
        "shell",
        json!({"command": "ls", "description": "List files"}),
    ));
    steps.push(Step::Permission(
        json!({"id": PERMISSION, "sessionID": SESSION, "action": "shell",
        "resources": ["ls"], "save": ["ls *"], "metadata": {"command": "ls"},
        "source": {"type": "tool", "messageID": "msg_a1", "id": "call_shell"}}),
    ));
    steps
}

/// What follows the answer `reply` to the permission.
pub(super) fn permitted(reply: &str) -> VecDeque<Step> {
    let mut steps = vec![event(
        "permission.replied",
        json!({"requestID": PERMISSION, "reply": reply}),
    )];
    if reply == "reject" {
        steps.push(tool_failed(
            "msg_a1",
            "call_shell",
            "permission.rejected",
            "The user rejected it",
        ));
    } else {
        steps.push(event(
            "session.tool.progress",
            on("msg_a1", json!({"id": "call_shell", "metadata": {"output": "a.txt\n"}})),
        ));
        steps.push(tool_success(
            "msg_a1",
            "call_shell",
            COMMAND_OUTPUT,
            Some(json!({"exit": 0})),
        ));
    }
    let patch = "@@ -1,2 +1,2 @@\n one\n-two\n+2\n";
    steps.extend(tool_called(
        "msg_a1",
        "call_edit",
        "edit",
        json!({"path": "a.txt", "oldString": "two", "newString": "2"}),
    ));
    steps.push(tool_success(
        "msg_a1",
        "call_edit",
        "Edited a.txt",
        Some(
            json!({"files": [{"file": "a.txt", "patch": patch, "additions": 1, "deletions": 1,
                               "status": "modified"}], "replacements": 1}),
        ),
    ));
    steps.push(step_ended(
        "msg_a1",
        "tool-calls",
        0.01,
        tokens(1000, 100, 20, 5000, 100),
    ));
    steps.extend(ending("msg_a2", "Done."));
    steps.into()
}

fn ending(message: &str, text: &str) -> Vec<Step> {
    let mut steps = vec![step_started(message)];
    steps.extend(streamed("text", message, 0, &[text]));
    steps.push(step_ended(message, "stop", 0.005, tokens(200, 30, 0, 6000, 0)));
    steps.push(event(
        "session.usage.updated",
        json!({"cost": 0.015, "tokens": tokens(1200, 130, 20, 11000, 100)}),
    ));
    steps.push(event("session.renamed", json!({"title": "Listing files"})));
    steps.push(event("session.execution.succeeded", json!({})));
    steps.push(event("session.idle", json!({})));
    steps
}

/// The end of the `gap` turn, which no client hears, and the messages the session keeps of the turn.
fn gap_ending(prompt: &str) -> Vec<Step> {
    let mut steps = streamed("text", "msg_a1", 1, &[", and done."]);
    steps.push(step_ended("msg_a1", "stop", 0.02, tokens(300, 40, 0, 0, 0)));
    steps.push(event("session.execution.succeeded", json!({})));
    steps.push(event("session.idle", json!({})));
    steps.push(Step::Record(json!([
        {"id": "msg_u1", "type": "user", "time": {"created": super::CLOCK}, "text": prompt},
        {"id": "msg_a1", "type": "assistant", "time": {"created": super::CLOCK + 100, "completed": super::CLOCK + 900},
         "agent": "build", "model": model(), "content": [{"type": "text", "text": super::GAP_ANSWER}]},
        {"id": "msg_i1", "type": "idle", "time": {"created": super::CLOCK + 1000}, "outcome": "succeeded"},
    ])));
    steps
}

fn question() -> Vec<Step> {
    let mut steps = tool_called(
        "msg_a1",
        "call_q",
        "question",
        json!({"questions": [{"question": "Which colour?"}]}),
    );
    steps.push(Step::Form(json!({"id": FORM, "sessionID": SESSION, "title": "Colour",
        "metadata": {"tool": {"id": "call_q"}},
        "fields": [{"key": "color", "type": "string", "title": "Which colour?",
                    "options": [{"value": "red", "label": "Red", "description": "warm"},
                                {"value": "blue", "label": "Blue", "description": "cold"}]}]})));
    steps
}

/// What follows the form's `answer`, or its cancellation with `None`.
pub(super) fn answered(answer: Option<&Value>) -> VecDeque<Step> {
    let Some(answer) = answer else {
        return vec![
            event("form.cancelled", json!({"id": FORM})),
            tool_failed("msg_a1", "call_q", "tool.execution", "The user dismissed the question"),
            event("session.execution.succeeded", json!({})),
        ]
        .into();
    };
    let color = answer["color"].as_str().unwrap_or("none").to_string();
    let mut steps = vec![
        event("form.replied", json!({"id": FORM, "answer": answer})),
        tool_success("msg_a1", "call_q", &color, None),
        step_ended("msg_a1", "tool-calls", 0.001, tokens(10, 1, 0, 0, 0)),
    ];
    steps.extend(ending("msg_a2", &format!("You chose {color}.")));
    steps.into()
}

/// The end of a turn the user interrupted.
pub(super) fn interrupted() -> VecDeque<Step> {
    vec![
        event("session.execution.interrupted", json!({"reason": "user"})),
        event("session.idle", json!({})),
    ]
    .into()
}
