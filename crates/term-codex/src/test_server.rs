//! A small stand-in for `codex app-server` in tests: it speaks the protocol line by line, refuses what the real
//! server refuses (requests before `initialize`, a second `initialize`), is stricter where a client could go wrong
//! (requests before `initialized`, a `"jsonrpc"` member), and plays fixed turns with every kind of notification and
//! request the mapper reads. It answers `--version` too, so the version check runs against it.
//!
//! The scenario comes from `CODEX_TEST_SCENARIO`: `basic`, `signin` (no account), `old` (an unsupported release).
//! A message containing `slow` streams one delta and waits for `turn/interrupt`; `crash` says [`CRASH`] on stderr
//! and exits mid-turn; `refuse` gets an error answer to `turn/start`; `ask` asks a question; `fail` ends the turn with an error. With `CODEX_TEST_LOG` set, every
//! line the client sends is appended to that file, and with `CODEX_TEST_ARGS` set, the command line.

use std::collections::VecDeque;
use std::io::{BufRead, Write};

use serde_json::{Value, json};

mod tasks;

/// The id of the thread the server opens.
pub const THREAD: &str = "thr_test";

/// The id of a stored thread `thread/list` names besides it, as one begun outside the app.
pub const OLDER_THREAD: &str = "thr_older";

/// The output of the command the user allowed.
pub const COMMAND_OUTPUT: &str = "a.txt\nb.txt";

/// What the server writes on stderr before it exits mid-turn.
pub const CRASH: &str = "the model crashed";

/// What `--version` prints, unless the scenario is `old`.
pub const VERSION: &str = "codex-cli 0.167.0";

/// The request ids of the server's requests.
const COMMAND_ASK: u64 = 900;
const FILE_ASK: u64 = 901;
const QUESTION_ASK: u64 = 902;

enum Step {
    Note(&'static str, Value),
    Ask(u64, &'static str, Value),
}

struct Server<W: Write> {
    out: W,
    scenario: String,
    initialized: bool,
    failing: bool,
    ready: bool,
    turns: u64,
    turn: Option<String>,
    steps: VecDeque<Step>,
    waiting: Option<u64>,
    exit: bool,
    /// Whether the client asked for the experimental API.
    experimental: bool,
    work: tasks::Work,
}

fn thread(turns: Value, name: Option<&str>) -> Value {
    json!({"id": THREAD, "sessionId": "ses_test", "cliVersion": "0.167.0", "createdAt": 1, "updatedAt": 2,
           "cwd": "/work", "ephemeral": false, "modelProvider": "openai", "preview": "", "projectId": null,
           "source": "appServer", "status": {"type": "idle"}, "turns": turns, "name": name})
}

impl<W: Write> Server<W> {
    fn send(&mut self, message: &Value) {
        let _ = writeln!(self.out, "{message}");
        let _ = self.out.flush();
    }

    fn reply(&mut self, id: &Value, result: Value) {
        self.send(&json!({"id": id, "result": result}));
    }

    fn error(&mut self, id: &Value, code: i64, message: &str) {
        self.send(&json!({"id": id, "error": {"code": code, "message": message}}));
    }

    fn note(&mut self, method: &str, mut params: Value) {
        if params.get("threadId").is_none() && method.starts_with(['i', 't']) && params.is_object() {
            params["threadId"] = json!(THREAD);
        }
        if let Some(turn) = &self.turn
            && method.starts_with("item/")
            && params.get("turnId").is_none()
        {
            params["turnId"] = json!(turn);
        }
        self.send(&json!({"method": method, "params": params, "emittedAtMs": 1}));
    }

    fn turn_value(&self, status: &str) -> Value {
        json!({"id": self.turn, "items": [], "status": status, "durationMs": 1200})
    }

    fn opened(&mut self, id: &Value, params: &Value, resumed: bool) {
        let turns = if resumed {
            json!([{"id": "turn-0", "status": "completed", "startedAt": 1, "items": [
                {"type": "userMessage", "id": "h1", "content": [{"type": "text", "text": "Earlier question"}]},
                {"type": "agentMessage", "id": "h2", "text": "Earlier answer"}]}])
        } else {
            json!([])
        };
        let result = json!({"thread": thread(turns, resumed.then_some("Earlier")),
            "model": params.get("model").cloned().unwrap_or(json!("gpt-5.5")), "modelProvider": "openai",
            "reasoningEffort": "medium", "cwd": params.get("cwd").cloned().unwrap_or(json!("/work")),
            "approvalPolicy": params.get("approvalPolicy").cloned().unwrap_or(json!("on-request")),
            "approvalsReviewer": "user", "sandbox": {"type": "workspaceWrite"}});
        self.reply(id, result);
        self.note("thread/started", json!({"thread": thread(json!([]), None)}));
    }

    fn turn_start(&mut self, id: &Value, params: &Value) {
        let input = params.get("input").cloned().unwrap_or(json!([]));
        let text: String = input
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|i| i["text"].as_str())
            .collect();
        if text.contains("refuse") {
            self.error(id, -32600, "the turn was refused");
            return;
        }
        self.turns += 1;
        self.turn = Some(format!("turn-{}", self.turns));
        let started = self.turn_value("inProgress");
        self.reply(id, json!({"turn": started}));
        self.note("turn/started", json!({"turn": started}));
        let user = json!({"type": "userMessage", "id": "u1", "content": input,
                          "clientId": params.get("clientUserMessageId")});
        self.note("item/started", json!({"item": user, "startedAtMs": 1000}));
        self.note("item/completed", json!({"item": user, "completedAtMs": 1000}));
        if text.contains("tasks") {
            self.task_turn(text.contains("early"), text.contains("late"));
            return;
        }
        if text.contains("crash") {
            self.note("item/agentMessage/delta", json!({"itemId": "m1", "delta": "Working"}));
            eprintln!("{CRASH}");
            self.exit = true;
            return;
        }
        if text.contains("slow") {
            self.note("item/agentMessage/delta", json!({"itemId": "m1", "delta": "Working"}));
            return;
        }
        self.failing = text.contains("fail");
        self.steps = if text.contains("ask") {
            question()
        } else if self.failing {
            failing()
        } else {
            opening()
        };
        self.play();
    }

    /// Ends the turn once its steps are played and nothing waits for the client.
    fn finish_if_done(&mut self) {
        if self.steps.is_empty() && self.waiting.is_none() && self.turn.is_some() {
            let status = if self.failing { "failed" } else { "completed" };
            self.end_turn(status);
        }
    }

    fn play(&mut self) {
        while let Some(step) = self.steps.pop_front() {
            match step {
                Step::Note(method, params) => self.note(method, params),
                Step::Ask(id, method, mut params) => {
                    params["threadId"] = json!(THREAD);
                    params["turnId"] = json!(self.turn);
                    self.send(&json!({"id": id, "method": method, "params": params}));
                    self.waiting = Some(id);
                    return;
                }
            }
        }
        self.finish_if_done();
    }

    fn end_turn(&mut self, status: &str) {
        let turn = self.turn_value(status);
        self.note("turn/completed", json!({"turn": turn}));
        self.turn = None;
        self.steps.clear();
        self.waiting = None;
    }

    fn answered(&mut self, id: u64, message: &Value) {
        if self.waiting != Some(id) {
            return;
        }
        self.waiting = None;
        let result = &message["result"];
        let decision = result["decision"].as_str().unwrap_or("");
        let accepted = matches!(decision, "accept" | "acceptForSession") || result["decision"].is_object();
        if decision == "cancel" {
            self.end_turn("interrupted");
            return;
        }
        let mut next = match id {
            COMMAND_ASK => command_done(accepted),
            FILE_ASK => file_done(accepted),
            _ => {
                let answer = result["answers"]["color"]["answers"][0]
                    .as_str()
                    .unwrap_or("none")
                    .to_string();
                message_steps("m3", &format!("You chose {answer}."))
            }
        };
        if id == FILE_ASK || id == QUESTION_ASK {
            next.extend(ending());
        }
        self.steps = next.into();
        self.play();
    }

    fn manage(&mut self, id: &Value, method: &str, params: &Value) -> bool {
        let result = match method {
            "account/read" if self.scenario == "signin" => json!({"account": null, "requiresOpenaiAuth": true}),
            "account/read" => json!({"account": {"type": "chatgpt", "email": "dev@example.test", "planType": "plus"},
                                     "requiresOpenaiAuth": true}),
            "model/list" => json!({"data": [
                {"id": "gpt-5.5", "model": "gpt-5.5", "displayName": "GPT-5.5", "description": "", "hidden": false,
                 "isDefault": true, "defaultReasoningEffort": "medium", "inputModalities": ["text", "image"],
                 "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": ""},
                                               {"reasoningEffort": "medium", "description": ""},
                                               {"reasoningEffort": "high", "description": ""}]},
                {"id": "gpt-5.5-mini", "model": "gpt-5.5-mini", "displayName": "Mini", "description": "",
                 "hidden": false, "isDefault": false, "defaultReasoningEffort": "low",
                 "supportedReasoningEfforts": [{"reasoningEffort": "low", "description": ""}]}],
                "nextCursor": null}),
            "thread/list" => {
                let mut older = thread(json!([]), Some("Begun in the terminal"));
                older["id"] = json!(OLDER_THREAD);
                json!({"data": [thread(json!([]), Some("Earlier")), older], "nextCursor": null})
            }
            "thread/revert" => json!({"thread": thread(json!([]), None)}),
            "thread/fork" => json!({"thread": {"id": "thr_fork", "preview": "", "cwd": "/work", "createdAt": 3,
                                    "updatedAt": 3, "forkedFromId": params["threadId"]}}),
            "config/read" => json!({"config": {"model": "gpt-5.5"}, "origins": {}}),
            "config/value/write" | "config/batchWrite" => {
                json!({"filePath": "/home/.codex/config.toml", "version": "v2", "status": "ok"})
            }
            "mcpServerStatus/list" => json!({"data": [{"name": "docs", "authStatus": "notLoggedIn",
                "tools": {"search": {}}, "resources": [], "resourceTemplates": []}], "nextCursor": null}),
            "mcpServer/oauth/login" => json!({"authorizationUrl": "https://auth.example.test/mcp", "loginId": "m1"}),
            "skills/list" => json!({"data": [{"cwd": "/work", "errors": [], "skills": [{"name": "review",
                "description": "Reviews", "path": "/work/.codex/skills/review/SKILL.md", "enabled": true,
                "scope": "repo"}]}]}),
            "skills/config/write" => json!({"effectiveEnabled": params["enabled"]}),
            "account/rateLimits/read" => json!({"rateLimits": {"primary": {"usedPercent": 42,
                "windowDurationMins": 300, "resetsAt": 2000000000}}}),
            "account/login/start" => json!({"type": "chatgpt", "authUrl": "https://auth.example.test/login",
                                            "loginId": "l1"}),
            "feedback/upload" => json!({"threadId": params["threadId"]}),
            "plugin/list" => json!({"marketplaces": []}),
            "config/mcpServer/reload" | "account/logout" | "account/login/cancel" => json!({}),
            _ => return false,
        };
        self.reply(id, result);
        if method == "account/login/start" {
            self.note("account/login/completed", json!({"loginId": "l1", "success": true}));
        }
        true
    }

    fn request(&mut self, id: &Value, method: &str, params: &Value) {
        if method == "initialize" {
            if self.initialized {
                return self.error(id, -32600, "Already initialized");
            }
            self.initialized = true;
            self.experimental = params["capabilities"]["experimentalApi"].as_bool() == Some(true);
            self.reply(
                id,
                json!({"userAgent": "dango/0.167.0", "codexHome": "/home/.codex", "platformFamily": "unix",
                       "platformOs": "macos"}),
            );
            return;
        }
        if !self.initialized {
            return self.error(id, -32600, "Not initialized");
        }
        if !self.ready {
            return self.error(id, -32600, "initialized was not sent");
        }
        if self.task_request(id, method, params) {
            return;
        }
        match method {
            "thread/start" => self.opened(id, params, false),
            "thread/resume" => self.opened(id, params, true),
            "turn/start" => self.turn_start(id, params),
            "turn/steer" => {
                self.reply(id, json!({"turnId": self.turn}));
                let user = json!({"type": "userMessage", "id": "u2", "content": params["input"],
                                  "clientId": params["clientUserMessageId"]});
                self.note("item/completed", json!({"item": user, "completedAtMs": 1000}));
            }
            "turn/interrupt" => {
                self.reply(id, json!({}));
                self.end_turn("interrupted");
            }
            _ if self.manage(id, method, params) => {}
            _ => self.error(id, -32600, &format!("Invalid request: unknown variant `{method}`")),
        }
    }

    fn handle(&mut self, message: &Value) {
        if message.get("jsonrpc").is_some() {
            eprintln!("protocol: the client sent a jsonrpc member");
            if let Some(id) = message.get("id") {
                self.error(id, -32600, "jsonrpc member sent");
            }
            return;
        }
        match (message.get("id"), message["method"].as_str()) {
            (Some(_), Some("test/no-reply")) => {}
            (Some(id), Some(method)) => self.request(&id.clone(), method, &message["params"]),
            (None, Some("initialized")) => self.ready = self.initialized,
            (Some(id), None) => {
                if let Some(id) = id.as_u64() {
                    self.answered(id, message);
                }
            }
            _ => {}
        }
    }
}

fn note(method: &'static str, params: Value) -> Step {
    Step::Note(method, params)
}

fn message_steps(id: &str, text: &str) -> Vec<Step> {
    vec![
        note(
            "item/started",
            json!({"item": {"type": "agentMessage", "id": id, "text": ""}, "startedAtMs": 1100}),
        ),
        note("item/agentMessage/delta", json!({"itemId": id, "delta": text})),
        note(
            "item/completed",
            json!({"item": {"type": "agentMessage", "id": id, "text": text}, "completedAtMs": 1200}),
        ),
    ]
}

fn plan(statuses: [&str; 2]) -> Step {
    note(
        "turn/plan/updated",
        json!({"plan": [{"step": "Read the file", "status": statuses[0]},
                                              {"step": "Fix it", "status": statuses[1]}]}),
    )
}

fn command(status: &str, output: Option<&str>) -> Value {
    json!({"type": "commandExecution", "id": "c1", "command": "ls -la", "cwd": "/work",
           "commandActions": [{"type": "unknown", "command": "ls -la"}], "status": status,
           "aggregatedOutput": output, "exitCode": output.map(|_| 0), "durationMs": output.map(|_| 5)})
}

fn file_change(status: &str) -> Value {
    json!({"type": "fileChange", "id": "f1", "status": status, "changes": [
        {"path": "/work/a.txt", "kind": {"type": "update", "move_path": null}, "diff": "@@ -1,2 +1,2 @@\n one\n-two\n+2\n"}]})
}

fn opening() -> VecDeque<Step> {
    let mut steps = vec![
        note(
            "item/started",
            json!({"item": {"type": "reasoning", "id": "r1", "summary": [], "content": []},
                                    "startedAtMs": 1000}),
        ),
        note(
            "item/reasoning/summaryPartAdded",
            json!({"itemId": "r1", "summaryIndex": 0}),
        ),
        note(
            "item/reasoning/summaryTextDelta",
            json!({"itemId": "r1", "summaryIndex": 0, "delta": "Reading "}),
        ),
        note(
            "item/reasoning/summaryTextDelta",
            json!({"itemId": "r1", "summaryIndex": 0, "delta": "the files."}),
        ),
        note(
            "item/completed",
            json!({"item": {"type": "reasoning", "id": "r1", "summary": ["Reading the files."],
                                      "content": []}, "completedAtMs": 3000}),
        ),
        note("thread/mystery", json!({"detail": "not in the protocol"})),
    ];
    steps.extend(message_steps("m1", "Let me look."));
    steps.extend([
        plan(["inProgress", "pending"]),
        note(
            "item/started",
            json!({"item": command("inProgress", None), "startedAtMs": 1300}),
        ),
        Step::Ask(
            COMMAND_ASK,
            "item/commandExecution/requestApproval",
            json!({"itemId": "c1",
            "command": "ls -la", "cwd": "/work", "reason": "list the folder", "proposedExecpolicyAmendment": ["ls"],
            "startedAtMs": 1300}),
        ),
    ]);
    steps.into()
}

fn command_done(accepted: bool) -> Vec<Step> {
    let mut steps = if accepted {
        vec![
            note(
                "item/commandExecution/outputDelta",
                json!({"itemId": "c1", "delta": "a.txt\n"}),
            ),
            note(
                "item/commandExecution/outputDelta",
                json!({"itemId": "c1", "delta": "b.txt"}),
            ),
            note(
                "item/completed",
                json!({"item": command("completed", Some(COMMAND_OUTPUT)), "completedAtMs": 1400}),
            ),
        ]
    } else {
        vec![note(
            "item/completed",
            json!({"item": command("declined", None), "completedAtMs": 1400}),
        )]
    };
    steps.extend([
        note(
            "item/started",
            json!({"item": file_change("inProgress"), "startedAtMs": 1500}),
        ),
        Step::Ask(
            FILE_ASK,
            "item/fileChange/requestApproval",
            json!({"itemId": "f1", "reason": "fix it",
            "startedAtMs": 1500}),
        ),
    ]);
    steps
}

fn file_done(accepted: bool) -> Vec<Step> {
    let status = if accepted { "completed" } else { "declined" };
    vec![
        note(
            "item/completed",
            json!({"item": file_change(status), "completedAtMs": 1600}),
        ),
        note(
            "turn/diff/updated",
            json!({"diff": "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+2\n"}),
        ),
    ]
}

fn ending() -> Vec<Step> {
    let mut steps = vec![
        plan(["completed", "completed"]),
        note(
            "thread/tokenUsage/updated",
            json!({"tokenUsage": {
            "total": {"inputTokens": 12000, "cachedInputTokens": 8000, "outputTokens": 600,
                      "reasoningOutputTokens": 200, "totalTokens": 12600},
            "last": {"inputTokens": 6000, "cachedInputTokens": 4000, "outputTokens": 300,
                     "reasoningOutputTokens": 100, "totalTokens": 6300},
            "modelContextWindow": 272000}}),
        ),
        note(
            "account/rateLimits/updated",
            json!({"rateLimits": {
            "primary": {"usedPercent": 85, "windowDurationMins": 300, "resetsAt": 2000000000},
            "secondary": {"usedPercent": 10, "windowDurationMins": 10080, "resetsAt": 2000500000}}}),
        ),
    ];
    steps.extend(message_steps("m2", "Done."));
    steps.push(note("thread/name/updated", json!({"threadName": "Listing files"})));
    steps
}

fn question() -> VecDeque<Step> {
    vec![Step::Ask(QUESTION_ASK, "item/tool/requestUserInput", json!({"itemId": "q1", "isBlocking": true,
        "questions": [{"id": "color", "header": "Colour", "question": "Which colour?",
                       "options": [{"label": "Red", "description": "warm"}, {"label": "Blue", "description": "cold"}]}]}))]
    .into()
}

fn failing() -> VecDeque<Step> {
    vec![note(
        "error",
        json!({"error": {"message": "You've hit your usage limit.",
                                        "codexErrorInfo": "usageLimitExceeded"}, "willRetry": false}),
    )]
    .into()
}

/// Runs as `codex` with the command line `args`, without the program, and the scenario and log of the environment:
/// `--version` or the app-server. The answer is the exit code.
pub fn main(args: &[String]) -> i32 {
    let scenario = std::env::var("CODEX_TEST_SCENARIO").unwrap_or_else(|_| "basic".into());
    if let Some(path) = std::env::var_os("CODEX_TEST_ARGS")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        // One write per line, since the version check and the server append at the same time.
        let _ = file.write_all(format!("{}\n", serde_json::json!(args)).as_bytes());
    }
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [.., "--version"] => {
            println!(
                "{}",
                if scenario == "old" {
                    "codex-cli 0.150.0"
                } else {
                    VERSION
                }
            );
        }
        _ => {
            let log = std::env::var_os("CODEX_TEST_LOG").map(std::path::PathBuf::from);
            run(std::io::stdin().lock(), std::io::stdout().lock(), &scenario, log);
        }
    }
    0
}

/// Plays `scenario` against the client on `input` and `output` until the input ends or a turn asks it to exit.
pub fn run(input: impl BufRead, output: impl Write, scenario: &str, log: Option<std::path::PathBuf>) {
    let mut server = Server::new(output, scenario);
    for line in input.lines().map_while(Result::ok) {
        if let Some(path) = &log
            && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            let _ = writeln!(file, "{line}");
        }
        if let Ok(message) = serde_json::from_str::<Value>(&line) {
            server.handle(&message);
        }
        if server.exit {
            return;
        }
    }
}

impl<W: Write> Server<W> {
    /// A server that writes to `output`.
    fn new(output: W, scenario: &str) -> Server<W> {
        Server {
            out: output,
            scenario: scenario.to_string(),
            initialized: false,
            failing: false,
            ready: false,
            turns: 0,
            turn: None,
            steps: VecDeque::new(),
            waiting: None,
            exit: false,
            experimental: false,
            work: tasks::Work::default(),
        }
    }
}
