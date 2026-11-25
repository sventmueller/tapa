//! Background terminals are an experimental management API, with no change notification. One worker per user
//! session polls complete, paginated snapshots. Errors keep the last known set; ending the session ends the worker.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::{Value, json};

use super::{After, Session, SessionError, Shared, now, reply};
use crate::mapping::Stop;
use crate::protocol::{method, text};
use crate::wire::Id;

const INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct EarlyRequest {
    pub(super) id: Id,
    method: String,
    params: Value,
}

impl Shared {
    /// A child may ask before its parent's spawn result proves its identity. Do not reject it or answer for
    /// another client: defer supported requests until the mapper knows that this thread belongs to us.
    pub(super) fn defer_task_request(&self, id: &Id, method: &str, params: &Value) -> bool {
        if !matches!(
            method,
            method::COMMAND_APPROVAL
                | method::FILE_CHANGE_APPROVAL
                | method::PERMISSIONS_APPROVAL
                | method::USER_INPUT
                | method::ELICITATION
        ) {
            return false;
        }
        let Some(thread) = text(params, "threadId") else {
            return false;
        };
        let mut state = self.state();
        if state.mapper.knows_task_thread(thread) || state.early_requests.len() >= 64 {
            return false;
        }
        state.early_requests.push(EarlyRequest {
            id: id.clone(),
            method: method.to_string(),
            params: params.clone(),
        });
        true
    }

    pub(super) fn replay_task_requests(self: &Arc<Self>) {
        let ready = {
            let mut state = self.state();
            let (ready, waiting): (Vec<_>, Vec<_>) =
                std::mem::take(&mut state.early_requests)
                    .into_iter()
                    .partition(|request| {
                        text(&request.params, "threadId").is_some_and(|id| state.mapper.knows_task_thread(id))
                    });
            state.early_requests = waiting;
            ready
        };
        for request in ready {
            self.on_request(request.id, &request.method, &request.params);
        }
    }
}

pub(super) fn watch(shared: &Arc<Shared>) {
    let weak = Arc::downgrade(shared);
    let spawned = std::thread::Builder::new().name("codex-tasks".into()).spawn(move || {
        loop {
            let Some(shared) = weak.upgrade() else { return };
            if shared.ended.load(Ordering::SeqCst) || shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            let threads = shared.state().mapper.task_threads();
            for thread in threads {
                if !terminals(&shared, &thread) {
                    return;
                }
            }
            drop(shared);
            std::thread::sleep(INTERVAL);
        }
    });
    if let Err(error) = spawned {
        shared.fail(SessionError::Start(error.to_string()));
    }
}

/// Returns false only when the connection or this optional API is unavailable. A partial page or a failure never
/// clears live tasks, and each complete snapshot is applied on the reader thread in protocol order.
fn terminals(shared: &Arc<Shared>, thread: &str) -> bool {
    let mut cursor = None::<String>;
    let mut seen = BTreeSet::new();
    let mut all = Vec::new();
    loop {
        let parent = thread.to_string();
        let mut collected = all.clone();
        let after: After = Box::new(move |result, shared| {
            if let Ok(result) = result
                && let Some(data) = result.get("data").and_then(Value::as_array)
                && result.get("nextCursor").is_none_or(Value::is_null)
            {
                collected.extend_from_slice(data);
                if !collected.iter().all(|terminal| {
                    ["processId", "itemId", "command"]
                        .iter()
                        .all(|field| text(terminal, field).is_some())
                }) {
                    return;
                }
                let updates = shared.state().mapper.background_terminals(&parent, &collected, now());
                shared.apply(updates);
            }
        });
        let answer = shared
            .call(
                method::BACKGROUND_LIST,
                json!({"threadId": thread, "cursor": cursor, "limit": 100}),
                Some(after),
                reply::raw,
            )
            .wait();
        let result = match answer {
            Ok(result) => result,
            Err(SessionError::Closed) => return false,
            Err(SessionError::Server(error))
                if error.code == -32601
                    || (error.code == -32600
                        && (error.message.contains("unknown variant")
                            || error.message.contains("experimentalApi"))) =>
            {
                return false;
            }
            Err(_) => return true,
        };
        let Some(data) = result.get("data").and_then(Value::as_array) else {
            return true;
        };
        all.extend_from_slice(data);
        cursor = text(&result, "nextCursor").map(str::to_string);
        match &cursor {
            Some(next) if seen.insert(next.clone()) => {}
            _ => return true,
        }
    }
}

impl Session {
    /// Stops one subagent turn or background terminal. Returns false if the task has already ended or is unknown.
    /// The main turn is left running, and a failed request keeps the task's last known state.
    pub fn stop_task(&self, task: &str) -> bool {
        if self.is_ended() {
            return false;
        }
        let Some(stop) = self.shared.state().mapper.task_stop(task) else {
            return false;
        };
        match stop {
            Stop::Agent { thread, turn } => {
                let after: After = Box::new(|result, shared| {
                    if let Err(error) = result {
                        shared.fail(SessionError::Server(error.clone()));
                    }
                });
                drop(self.shared.call(
                    method::TURN_INTERRUPT,
                    json!({"threadId": thread, "turnId": turn}),
                    Some(after),
                    reply::unit,
                ));
            }
            Stop::Process { thread, process } => {
                let task = task.to_string();
                let after: After = Box::new(move |result, shared| match result {
                    Ok(result) if result["terminated"].as_bool() == Some(true) => {
                        let updates = shared.state().mapper.process_stopped(&task, now());
                        shared.apply(updates);
                    }
                    Err(error) => shared.fail(SessionError::Server(error.clone())),
                    _ => {}
                });
                drop(self.shared.call(
                    method::BACKGROUND_TERMINATE,
                    json!({"threadId": thread, "processId": process}),
                    Some(after),
                    reply::unit,
                ));
            }
        }
        true
    }
}
