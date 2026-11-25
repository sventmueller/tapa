//! A live child thread and three PTY sessions; no command or model actually runs.

use std::collections::BTreeSet;
use std::io::Write;

use serde_json::{Value, json};

use super::{Server, THREAD};

const CHILD: &str = "thr_child";
const CHILD_TURN: &str = "child_turn";

#[derive(Default)]
pub(super) struct Work {
    running: BTreeSet<String>,
}

impl<W: Write> Server<W> {
    pub(super) fn task_turn(&mut self, early_ask: bool, late_ask: bool) {
        self.work.running = ["p1", "p2", "pc"].into_iter().map(str::to_string).collect();
        self.note(
            "turn/started",
            json!({"threadId": CHILD, "turn": {"id": CHILD_TURN, "status": "inProgress"}}),
        );
        self.note(
            "item/agentMessage/delta",
            json!({"threadId": CHILD, "turnId": CHILD_TURN, "itemId": "child_text", "delta": "Reviewing"}),
        );
        for (thread, turn, item, process) in [
            (THREAD, self.turn.clone(), "bg1", "p1"),
            (THREAD, self.turn.clone(), "bg2", "p2"),
            (CHILD, Some(CHILD_TURN.into()), "bgc", "pc"),
        ] {
            self.note("item/started", json!({"threadId": thread, "turnId": turn, "item": {
                "type": "commandExecution", "id": item, "processId": process, "command": "sleep 30", "cwd": "/work", "status": "inProgress"
            }}));
            self.note(
                "item/commandExecution/outputDelta",
                json!({"threadId": thread, "turnId": turn, "itemId": item, "delta": "started\n"}),
            );
        }
        if early_ask {
            self.send(
                &json!({"id": 910, "method": "item/commandExecution/requestApproval", "params": {
                    "threadId": CHILD, "turnId": CHILD_TURN, "itemId": "bgc", "command": "sleep 30"
                }}),
            );
        }
        self.note("item/completed", json!({"item": {
            "type": "collabAgentToolCall", "id": "spawn", "tool": "spawnAgent", "status": "completed",
            "prompt": "Review the files", "receiverThreadIds": [CHILD], "agentsStates": {CHILD: {"status": "running"}}
        }}));
        self.note(
            "thread/started",
            json!({"thread": {"id": CHILD, "parentThreadId": THREAD, "agentRole": "reviewer"}}),
        );
        if late_ask {
            self.end_turn("completed");
            self.send(
                &json!({"id": 910, "method": "item/commandExecution/requestApproval", "params": {
                    "threadId": CHILD, "turnId": CHILD_TURN, "itemId": "bgc", "command": "sleep 30"
                }}),
            );
        }
    }

    pub(super) fn task_request(&mut self, id: &Value, method: &str, params: &Value) -> bool {
        match method {
            "thread/backgroundTerminals/list" => {
                if !self.experimental {
                    self.error(id, -32600, "background terminals require experimentalApi");
                    return true;
                }
                let (keys, cursor): (&[(&str, &str)], Option<&str>) = if params["threadId"] == CHILD {
                    (&[("pc", "bgc")], None)
                } else if params["cursor"] == "second" {
                    (&[("p2", "bg2")], None)
                } else {
                    (&[("p1", "bg1")], (!self.work.running.is_empty()).then_some("second"))
                };
                let data: Vec<Value> = keys.iter().filter(|(p, _)| self.work.running.contains(*p))
                    .map(|(process, item)| json!({"processId": process, "itemId": item, "command": "sleep 30", "cwd": "/work"})).collect();
                self.reply(id, json!({"data": data, "nextCursor": cursor}));
                true
            }
            "thread/backgroundTerminals/terminate" => {
                let process = params["processId"].as_str().unwrap_or_default();
                let removed = self.work.running.remove(process);
                self.reply(id, json!({"terminated": removed}));
                true
            }
            "turn/interrupt" if params["threadId"] == CHILD => {
                self.reply(id, json!({}));
                self.note(
                    "turn/completed",
                    json!({"threadId": CHILD, "turn": {"id": CHILD_TURN, "status": "interrupted"}}),
                );
                true
            }
            _ => false,
        }
    }
}
