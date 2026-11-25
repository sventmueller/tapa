//! Plays a recorded session into a thread without a process, for tests and the self-test's AI tab.
//!
//! A recording is the CLI's NDJSON output; lines starting with `>> ` are what the app sent at that point. The app's
//! answers to permission requests and its control requests are taken from those lines, so a replay ends in the
//! state the live session reached.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use term_thread::{Answer as QuestionAnswer, Change, Decision, Thread, Update, UserMessage};

use crate::control::{
    self, Answer, ContextCount, ContextUsage, Incoming, InitializeResult, PermissionAsk, PlanAnswer, TaskOutput,
    UsageReport,
};
use crate::mapping::Mapper;
use crate::protocol::{self, Message, Outcome};

/// The prefix of a line the app sent.
pub const SENT: &str = ">> ";

/// Where a request the app sent keeps how many prompts came before it.
const PROMPTS: &str = "@prompts";

/// A recording being played into a thread.
pub struct Replay {
    mapper: Mapper,
    thread: Thread,
    asks: HashMap<String, PermissionAsk>,
    sent: HashMap<String, Value>,
    clock: SystemTime,
    step: Duration,
}

impl Replay {
    /// A replay into `thread` whose first line happens at `start`; each further line is `step` later.
    pub fn new(thread: Thread, start: SystemTime, step: Duration) -> Replay {
        Replay {
            mapper: Mapper::new(),
            thread,
            asks: HashMap::new(),
            sent: HashMap::new(),
            clock: start,
            step,
        }
    }

    /// The same replay of a resumed session, whose totals the CLI may count from an earlier process.
    pub fn resumed(mut self) -> Replay {
        self.mapper.resumed();
        self
    }

    /// Plays every line of `recording`.
    pub fn feed_all(&mut self, recording: &str) -> Vec<Change> {
        recording.lines().flat_map(|line| self.feed(line)).collect()
    }

    /// Plays one line.
    pub fn feed(&mut self, line: &str) -> Vec<Change> {
        let now = self.clock;
        self.clock += self.step;
        let updates = match line.strip_prefix(SENT) {
            Some(frame) => serde_json::from_str::<Value>(frame).map_or_else(|_| Vec::new(), |v| self.sent(&v, now)),
            None => match protocol::parse(line) {
                Ok(message) => self.received(message, now),
                Err(_) => Vec::new(),
            },
        };
        self.thread.apply_all(updates)
    }

    /// Answers the plan request `request_id` with the frame a session would send, as if it had been recorded.
    pub fn answer_plan(&mut self, request_id: &str, answer: &PlanAnswer) -> Vec<Change> {
        let Some(ask) = self.asks.get(request_id) else {
            return Vec::new();
        };
        let frame = control::success_frame(request_id, answer.permission().payload(ask));
        self.feed(&format!("{SENT}{frame}"))
    }

    /// The thread as it is now.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }

    /// The thread, once the replay is done.
    pub fn into_thread(self) -> Thread {
        self.thread
    }

    fn sent(&mut self, frame: &Value, now: SystemTime) -> Vec<Update> {
        let id = |v: &Value| {
            v.get("request_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match frame.get("type").and_then(Value::as_str) {
            Some("user") => {
                let uuid = frame.get("uuid").and_then(Value::as_str).unwrap_or_default();
                let text = frame
                    .pointer("/message/content")
                    .map(crate::tools::result_text)
                    .unwrap_or_default();
                let attachments = frame
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|block| match serde_json::from_value(block.clone()) {
                        Ok(protocol::Block::Image { source: Some(source) }) => crate::mapping::image(&source),
                        _ => None,
                    })
                    .collect();
                let message = UserMessage {
                    text,
                    attachments,
                    ..UserMessage::default()
                };
                self.mapper.user_message(message, uuid, now)
            }
            Some("control_request") => {
                let mut request = frame.get("request").cloned().unwrap_or_default();
                if let Value::Object(map) = &mut request {
                    map.insert(PROMPTS.into(), Value::from(self.mapper.prompts()));
                }
                self.sent.insert(id(frame), request);
                Vec::new()
            }
            Some("control_response") => {
                let response = frame.get("response").cloned().unwrap_or_default();
                let request = id(&response);
                let Some(ask) = self.asks.remove(&request) else {
                    return Vec::new();
                };
                let decision = decision(&ask, response.get("response").unwrap_or(&Value::Null));
                self.mapper.decided(&request, decision)
            }
            _ => Vec::new(),
        }
    }

    fn received(&mut self, message: Message, now: SystemTime) -> Vec<Update> {
        match message {
            Message::ControlRequest { request_id, request } => match Incoming::read(&request) {
                Incoming::CanUseTool(ask) => {
                    let updates = self.mapper.permission(&request_id, &ask, now);
                    self.asks.insert(request_id, *ask);
                    updates
                }
                Incoming::HookCallback { .. } | Incoming::Other { .. } => Vec::new(),
            },
            Message::ControlResponse(response) => {
                let Some(request) = self.sent.remove(&response.request_id) else {
                    return Vec::new();
                };
                let payload = response.response.unwrap_or_default();
                if response.subtype != Outcome::Success {
                    return Vec::new();
                }
                let field = |name: &str| {
                    request
                        .get(name)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                match request.get("subtype").and_then(Value::as_str) {
                    Some("initialize") => InitializeResult::read(payload)
                        .map(|result| vec![result.billing()])
                        .unwrap_or_default(),
                    Some("get_task_output") => TaskOutput::read(payload)
                        .map(|output| self.mapper.task_output(&field("task_id"), &output))
                        .unwrap_or_default(),
                    Some("get_context_usage") => ContextUsage::read(payload)
                        .map(|usage| {
                            let count = if field("detail") == "full" {
                                ContextCount::Exact
                            } else {
                                ContextCount::Estimated
                            };
                            self.mapper.context(&usage, count, now)
                        })
                        .unwrap_or_default(),
                    Some("get_usage") => {
                        if let Ok(report) = UsageReport::read(payload) {
                            let prompts = request.get(PROMPTS).and_then(Value::as_u64).unwrap_or_default();
                            self.mapper.baseline(&report, prompts);
                        }
                        Vec::new()
                    }
                    Some("set_permission_mode") => vec![Update::Mode(field("mode"))],
                    Some("set_model") => vec![Update::Model(field("model"))],
                    _ => Vec::new(),
                }
            }
            Message::ControlCancel { request_id } => {
                self.asks.remove(&request_id);
                self.mapper.withdrawn(&request_id)
            }
            other => self.mapper.map(&other, now),
        }
    }
}

/// The decision an answer to `ask` stands for.
fn decision(ask: &PermissionAsk, response: &Value) -> Decision {
    let allowed = response.get("behavior").and_then(Value::as_str) == Some("allow");
    let message = response.get("message").and_then(Value::as_str).map(str::to_string);
    match (ask.tool_name.as_str(), allowed) {
        ("AskUserQuestion", true) => Decision::Answered(
            response
                .pointer("/updatedInput/answers")
                .and_then(Value::as_object)
                .map(|answers| {
                    answers
                        .iter()
                        .map(|(question, answer)| QuestionAnswer {
                            question: question.clone(),
                            answer: answer.as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        ("ExitPlanMode", true) => Decision::Approved,
        ("ExitPlanMode", false) => control::plan_rejection(
            message.as_deref(),
            response.get("interrupt").and_then(Value::as_bool) == Some(true),
        ),
        (_, true) => Decision::Allowed {
            always: response
                .get("updatedPermissions")
                .and_then(Value::as_array)
                .is_some_and(|rules| !rules.is_empty()),
            edited: response.get("updatedInput").is_some_and(|input| *input != ask.input),
        },
        (_, false) => Decision::Denied { message },
    }
}
