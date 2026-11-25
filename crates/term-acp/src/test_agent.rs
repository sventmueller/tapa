//! A small ACP agent for tests and the self-test: it speaks the protocol line by line without the SDK, so it also
//! checks what the client puts on the wire, and plays a fixed turn with every kind of update the app shows.
//!
//! Scenarios: `basic`; `auth`, which wants a sign-in before a session; `v2`, which only speaks protocol version 2.
//! A prompt containing `slow` streams one chunk and waits to be cancelled; one containing `crash` streams a chunk, says
//! [`CRASH`] on stderr and exits mid-turn; one containing `quit` ends its turn and then exits. Every prompt comes back as
//! a user message chunk first, as Antigravity sends it. It forks a session for a question beside the conversation,
//! whose answer asks for a tool the client must refuse, and signs out on request. With `ACP_TEST_RECORD` set it
//! appends to that file the MCP servers it was handed, the sign-out and the client's answer in the fork.

use std::collections::VecDeque;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The session id the agent hands out.
pub const SESSION: &str = "test-session-1";

/// What the agent's turn reports after the user allowed the command.
pub const COMMAND_OUTPUT: &str = "a.txt\nb.txt";

/// What the agent writes on stderr before it exits mid-turn.
pub const CRASH: &str = "the model crashed";

/// The request id of the permission request.
const PERMISSION_ID: u64 = 900;

/// The request id of the file read the client must refuse.
const FS_ID: u64 = 901;

/// The request id of the permission request in a fork.
const ASIDE_ID: u64 = 902;

/// The id of the fork the agent hands out.
pub const FORK: &str = "test-fork-1";

/// What the agent answers in a fork, after `Beside: ` and the question.
pub const ASIDE_ANSWER: &str = "Beside: ";

enum Step {
    /// A `session/update` with this update.
    Update(Value),
    /// Writes a file in the working folder, as an agent's own edit tool does.
    Write(&'static str, &'static str),
    /// A request to the client; the turn waits for its answer.
    Ask(u64, &'static str, Value),
}

struct Agent<W: Write> {
    out: W,
    scenario: String,
    authed: bool,
    prompt: Option<Value>,
    waiting: Option<u64>,
    steps: VecDeque<Step>,
    exit: bool,
    /// The prompt in the fork, which waits for the client's answer to its permission request.
    aside: Option<Value>,
}

fn update(update: Value) -> Step {
    Step::Update(update)
}

fn chunk(kind: &str, text: &str) -> Step {
    update(json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}}))
}

fn plan(statuses: [&str; 2]) -> Step {
    update(json!({"sessionUpdate": "plan", "entries": [
        {"content": "Read the file", "priority": "high", "status": statuses[0]},
        {"content": "Fix it", "priority": "medium", "status": statuses[1]}
    ]}))
}

fn model_option(current: &str) -> Value {
    json!({"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": current,
           "options": [{"value": "fast", "name": "Fast"}, {"value": "deep", "name": "Deep"}]})
}

/// The turn up to the permission request.
fn opening() -> Vec<Step> {
    vec![
        chunk("agent_thought_chunk", "Thinking about it."),
        chunk("agent_message_chunk", "Let me "),
        chunk("agent_message_chunk", "look."),
        plan(["in_progress", "pending"]),
        update(
            json!({"sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "ls -la", "kind": "execute",
                      "status": "pending", "rawInput": {"command": "ls -la"}}),
        ),
        Step::Ask(
            PERMISSION_ID,
            "session/request_permission",
            json!({"sessionId": SESSION, "toolCall": {"toolCallId": "call-1"}, "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
                {"optionId": "always", "name": "Always allow ls", "kind": "allow_always"},
                {"optionId": "reject", "name": "Reject", "kind": "reject_once"}
            ]}),
        ),
    ]
}

/// The file the agent's edit writes in the working folder, and its content before and after.
pub const EDITED: (&str, &str, &str) = ("a.txt", "one\ntwo\n", "one\n2\n");

fn edited_path() -> String {
    std::env::current_dir()
        .unwrap_or_default()
        .join(EDITED.0)
        .to_string_lossy()
        .into_owned()
}

/// The rest of the turn once the user answered: `allowed` says whether the command runs.
fn closing(allowed: bool) -> Vec<Step> {
    let mut steps = if allowed {
        vec![
            update(json!({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "in_progress"})),
            update(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed",
                          "content": [{"type": "content", "content": {"type": "text", "text": COMMAND_OUTPUT}}]}),
            ),
        ]
    } else {
        vec![update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "failed"}),
        )]
    };
    steps.extend([
        Step::Write(EDITED.0, EDITED.2),
        update(
            json!({"sessionUpdate": "tool_call", "toolCallId": "call-2", "title": "Edit a.txt", "kind": "edit",
                      "status": "completed", "content": [{"type": "diff", "path": edited_path(),
                      "oldText": EDITED.1, "newText": EDITED.2}]}),
        ),
        update(json!({"sessionUpdate": "mystery_update", "detail": "not in the protocol"})),
        Step::Ask(
            FS_ID,
            "fs/read_text_file",
            json!({"sessionId": SESSION, "path": "/etc/hosts"}),
        ),
    ]);
    steps
}

fn ending(fs_refused: bool) -> Vec<Step> {
    vec![
        plan(["completed", "completed"]),
        update(json!({"sessionUpdate": "usage_update", "used": 12000, "size": 200000,
                      "cost": {"amount": 0.0123, "currency": "USD"}})),
        update(json!({"sessionUpdate": "session_info_update", "title": "Listing files"})),
        chunk(
            "agent_message_chunk",
            if fs_refused {
                "Done; reading files was refused."
            } else {
                "Done; reading files was allowed."
            },
        ),
    ]
}

impl<W: Write> Agent<W> {
    fn send(&mut self, message: Value) {
        let _ = writeln!(self.out, "{message}");
        let _ = self.out.flush();
    }

    fn reply(&mut self, id: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&mut self, id: &Value, code: i64, message: &str) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    fn notify(&mut self, update: Value) {
        self.notify_in(SESSION, update);
    }

    fn notify_in(&mut self, session: &str, update: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": "session/update",
                         "params": {"sessionId": session, "update": update}}));
    }

    fn record(line: &str) {
        if let Some(path) = std::env::var_os("ACP_TEST_RECORD") {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(path);
            if let Ok(mut file) = file {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    /// Answers a question in the fork: its words, then a tool call the client must refuse.
    fn ask_aside(&mut self, id: Value, text: &str) {
        self.notify_in(
            FORK,
            json!({"sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": format!("{ASIDE_ANSWER}{text}")}}),
        );
        self.send(
            json!({"jsonrpc": "2.0", "id": ASIDE_ID, "method": "session/request_permission",
            "params": {"sessionId": FORK, "toolCall": {"toolCallId": "aside-1"}, "options": [
                {"optionId": "allow", "name": "Allow", "kind": "allow_once"}]}}),
        );
        self.aside = Some(id);
    }

    fn initialize(&mut self, id: &Value) {
        let version = if self.scenario == "v2" { 2 } else { 1 };
        let auth = if self.scenario == "auth" {
            json!([{"id": "browser", "name": "Sign in with the browser", "description": "Opens the browser"}])
        } else {
            json!([])
        };
        self.reply(
            id,
            json!({"protocolVersion": version, "authMethods": auth,
                   "agentInfo": {"name": "acp-test-agent", "title": "Test Agent", "version": "1.0.0"},
                   "agentCapabilities": {"loadSession": true,
                                         "promptCapabilities": {"image": true, "embeddedContext": true},
                                         "mcpCapabilities": {"http": true},
                                         "auth": {"logout": {}},
                                         "sessionCapabilities": {"fork": {}, "close": {}},
                                         "_meta": {"test.vendor": {"on": true}}}}),
        );
    }

    fn new_session(&mut self, id: &Value) {
        if self.scenario == "auth" && !self.authed {
            self.error(id, -32000, "Authentication required");
            return;
        }
        self.reply(
            id,
            json!({"sessionId": SESSION,
                   "modes": {"currentModeId": "ask", "availableModes": [
                       {"id": "ask", "name": "Ask", "description": "Asks before changes"},
                       {"id": "code", "name": "Code"}]},
                   "configOptions": [model_option("fast")]}),
        );
        self.notify(
            json!({"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "review", "description": "Review the changes", "input": {"hint": "what to review"}}]}),
        );
    }

    fn load_session(&mut self, id: &Value) {
        self.notify(json!({"sessionUpdate": "user_message_chunk",
                           "content": {"type": "text", "text": "Earlier question"}}));
        self.notify(json!({"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": "Earlier answer"}}));
        self.reply(
            id,
            json!({"modes": {"currentModeId": "ask", "availableModes": [{"id": "ask", "name": "Ask"}, {"id": "code", "name": "Code"}]},
                   "configOptions": [model_option("deep")]}),
        );
    }

    fn prompt(&mut self, id: Value, params: &Value) {
        let text: String = params["prompt"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| b["text"].as_str())
            .collect();
        if params["sessionId"] == FORK {
            return self.ask_aside(id, &text);
        }
        self.prompt = Some(id);
        self.notify(json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": text}}));
        if self.scenario.starts_with("usage-") {
            self.usage_reply();
            return;
        }
        if text.contains("crash") {
            self.notify(json!({"sessionUpdate": "agent_message_chunk",
                               "content": {"type": "text", "text": "Working on it"}}));
            eprintln!("{CRASH}");
            self.exit = true;
            return;
        }
        if text.contains("quit") {
            if let Some(id) = self.prompt.take() {
                self.reply(&id, json!({"stopReason": "end_turn"}));
            }
            self.exit = true;
            return;
        }
        if text.contains("slow") {
            self.notify(json!({"sessionUpdate": "agent_message_chunk",
                               "content": {"type": "text", "text": "Working on it"}}));
            return;
        }
        self.steps = opening().into();
        self.play();
    }

    fn usage_reply(&mut self) {
        if self.scenario == "usage-credits" {
            self.notify(json!({"sessionUpdate": "usage_update", "used": 12000, "size": 200000,
                "cost": {"amount": 2.5, "currency": "credits"}}));
        }
        let result = if self.scenario == "usage-metadata" {
            json!({"stopReason": "end_turn", "_meta": {"quota": {
                "token_count": {"input_tokens": 700, "output_tokens": 300}
            }}})
        } else {
            json!({"stopReason": "end_turn", "usage": {
                "totalTokens": 1000, "inputTokens": 700, "outputTokens": 300,
                "cachedReadTokens": 200, "thoughtTokens": 100
            }})
        };
        if let Some(id) = self.prompt.take() {
            self.reply(&id, result);
        }
    }

    /// Plays steps until one waits for the client or the turn is over.
    fn play(&mut self) {
        while let Some(step) = self.steps.pop_front() {
            match step {
                Step::Update(update) => self.notify(update),
                Step::Write(name, content) => {
                    let _ = std::fs::write(name, content);
                }
                Step::Ask(id, method, params) => {
                    self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
                    self.waiting = Some(id);
                    return;
                }
            }
        }
        if let Some(id) = self.prompt.take() {
            self.reply(&id, json!({"stopReason": "end_turn"}));
        }
    }

    fn answered(&mut self, id: u64, message: &Value) {
        if self.waiting != Some(id) {
            return;
        }
        self.waiting = None;
        if id == PERMISSION_ID {
            let outcome = &message["result"]["outcome"];
            let chosen = outcome["optionId"].as_str().unwrap_or_default();
            let allowed = outcome["outcome"] == "selected" && chosen != "reject";
            self.steps = closing(allowed).into();
        } else {
            self.steps = ending(message["error"]["code"] == -32601).into();
        }
        self.play();
    }

    fn request(&mut self, id: Value, method: &str, params: &Value) {
        match method {
            "initialize" => self.initialize(&id),
            "authenticate" => {
                self.authed = params["methodId"] == "browser";
                self.reply(&id, json!({}));
            }
            "session/new" => {
                Self::record(&format!("mcpServers: {}", params["mcpServers"]));
                self.new_session(&id);
            }
            "session/load" => {
                Self::record(&format!("mcpServers: {}", params["mcpServers"]));
                self.load_session(&id);
            }
            "session/fork" => self.reply(&id, json!({"sessionId": FORK})),
            "session/close" => {
                Self::record(&format!("closed: {}", params["sessionId"]));
                self.reply(&id, json!({}));
            }
            "logout" => {
                Self::record("logout");
                self.reply(&id, json!({}));
            }
            "session/prompt" => self.prompt(id, params),
            "session/set_mode" => {
                let mode = params["modeId"].clone();
                self.reply(&id, json!({}));
                self.notify(json!({"sessionUpdate": "current_mode_update", "currentModeId": mode}));
            }
            "session/set_config_option" => {
                let value = params["value"].as_str().unwrap_or("fast").to_string();
                self.reply(&id, json!({"configOptions": [model_option(&value)]}));
            }
            _ => self.error(&id, -32601, "Method not found"),
        }
    }

    fn handle(&mut self, message: &Value) {
        let method = message["method"].as_str();
        match (message.get("id"), method) {
            (Some(id), Some(method)) => self.request(id.clone(), method, &message["params"]),
            (None, Some("session/cancel")) => {
                self.steps.clear();
                self.waiting = None;
                if let Some(id) = self.prompt.take() {
                    self.reply(&id, json!({"stopReason": "cancelled"}));
                }
            }
            (Some(id), None) if id.as_u64() == Some(ASIDE_ID) => {
                Self::record(&format!(
                    "aside permission: {}",
                    message["result"]["outcome"]["outcome"]
                ));
                if let Some(id) = self.aside.take() {
                    self.reply(&id, json!({"stopReason": "end_turn"}));
                }
            }
            (Some(id), None) => {
                if let Some(id) = id.as_u64() {
                    self.answered(id, message);
                }
            }
            _ => {}
        }
    }
}

/// Plays `scenario` against the client on `input` and `output` until the input ends or a prompt asks it to exit.
pub fn run(input: impl BufRead, output: impl Write, scenario: &str) {
    let mut agent = Agent {
        out: output,
        scenario: scenario.to_string(),
        authed: false,
        prompt: None,
        waiting: None,
        steps: VecDeque::new(),
        exit: false,
        aside: None,
    };
    for line in input.lines().map_while(Result::ok) {
        if let Ok(message) = serde_json::from_str::<Value>(&line) {
            agent.handle(&message);
        }
        if agent.exit {
            return;
        }
    }
}
