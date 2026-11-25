//! The event stream: `GET /api/event` as server-sent events, one `V2Event` per event. The server reports every
//! location's events on it, so the mapper keeps to the session's; permissions and forms become requests that wait
//! for the user, and changed agents or models are read anew.
//!
//! The stream is volatile by the schema's contract: events while it is down are missed. When it breaks while the
//! server runs, the session reports the gap and opens it again a few times before it gives up; `Session::resync`
//! catches up on what the gap swallowed.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::Value;
use term_thread::{Decision, RequestState, Status, Update};

use super::{Event, ServerError, SessionError, Shared, lock, now};
use crate::http::Events;
use crate::mapping::text;
use crate::requests::{self, Pick};

/// How often a broken stream is opened again.
const RECONNECTS: u32 = 5;

/// How long to wait before opening it again.
const RECONNECT_PAUSE: Duration = Duration::from_millis(300);

fn connect(shared: &Shared) -> Result<Events, SessionError> {
    match shared.endpoint()?.events("/api/event").map_err(SessionError::Http)? {
        Ok(events) => {
            if let Ok(closer) = events.closer() {
                *lock(&shared.stream) = Some(closer);
            }
            Ok(events)
        }
        Err(response) => Err(SessionError::Server(ServerError::from_response(&response))),
    }
}

/// Opens the stream and reads it on a thread of its own until the session ends.
pub(super) fn subscribe(shared: &Arc<Shared>) -> Result<(), SessionError> {
    let events = connect(shared)?;
    let reader = shared.clone();
    std::thread::Builder::new()
        .name("opencode-events".into())
        .spawn(move || read(&reader, events))
        .map(drop)
        .map_err(|e| SessionError::Start(e.to_string()))
}

fn read(shared: &Arc<Shared>, mut events: Events) {
    let mut left = RECONNECTS;
    loop {
        while let Some(event) = events.next_event() {
            left = RECONNECTS;
            if let Ok(value) = serde_json::from_str::<Value>(&event.data) {
                on_event(shared, &value);
            }
        }
        if shared.is_ended() || shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        shared.emit(Event::StreamGap);
        let mut error = SessionError::Closed;
        let reopened = loop {
            if left == 0 {
                break None;
            }
            left -= 1;
            std::thread::sleep(RECONNECT_PAUSE);
            if shared.is_ended() || shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            match connect(shared) {
                Ok(events) => break Some(events),
                Err(e) => error = e,
            }
        };
        match reopened {
            Some(reopened) => {
                events = reopened;
                shared.emit(Event::StreamResumed);
            }
            None => {
                shared.fail(error);
                shared.stopping.store(true, Ordering::SeqCst);
                shared.close();
                shared.supervisor.stop(shared.process, Duration::from_millis(500));
                return;
            }
        }
    }
}

fn ours(shared: &Shared, session: Option<&str>) -> bool {
    let own = shared.state().session_id.clone();
    session.is_some() && own.as_deref() == session
}

/// One event of the stream.
pub(super) fn on_event(shared: &Arc<Shared>, event: &Value) {
    let kind = text(event, "type").unwrap_or_default();
    let data = event.get("data").unwrap_or(&Value::Null);
    match kind {
        "permission.asked" => return asked(shared, data, false),
        "form.created" => return asked(shared, data.get("form").unwrap_or(&Value::Null), true),
        "permission.replied" => {
            let decision = match text(data, "reply") {
                Some("once") => Pick::Once.decision(),
                Some("always") => Pick::Always.decision(),
                _ => Pick::Reject.decision(),
            };
            return settled(shared, text(data, "requestID"), RequestState::Decided(decision));
        }
        "form.replied" => {
            return settled(
                shared,
                text(data, "id"),
                RequestState::Decided(Decision::Answered(Vec::new())),
            );
        }
        "form.cancelled" => return settled(shared, text(data, "id"), RequestState::Withdrawn),
        "mcp.status.changed" => {
            if let Some(server) = text(data, "server") {
                shared.emit(Event::McpChanged(server.to_string()));
            }
            return;
        }
        "agent.updated"
        | "model.updated"
        | "provider.updated"
        | "models-dev.refreshed"
        | "credential.updated"
        | "credential.switched"
        | "integration.updated" => return refresh(shared),
        _ => {}
    }
    let (updates, learned) = {
        let mut state = shared.state();
        let super::State { mapper, caps, .. } = &mut *state;
        mapper.event(event, caps, now())
    };
    shared.apply(updates);
    if learned {
        shared.emit(Event::Capabilities);
    }
    if kind == "session.execution.started" && std::mem::take(&mut shared.state().interrupt) {
        shared.interrupt();
    }
}

/// A permission or form of the session, which waits for the user from now on.
fn asked(shared: &Shared, data: &Value, form: bool) {
    if !ours(shared, text(data, "sessionID")) {
        return;
    }
    let updates = {
        let mut state = shared.state();
        let made = if form {
            state.mapper.form(data, now())
        } else {
            state.mapper.permission(data, now())
        };
        let Some((id, offer, updates)) = made else {
            return;
        };
        if state.asks.contains_key(&id) {
            return;
        }
        state.asks.insert(id, offer);
        updates
    };
    shared.apply(updates);
}

/// A request that was answered or withdrawn elsewhere, such as by OpenCode when its turn ended.
fn settled(shared: &Shared, id: Option<&str>, outcome: RequestState) {
    let updates = {
        let mut state = shared.state();
        let Some(offer) = id.and_then(|id| state.asks.remove(id)) else {
            return;
        };
        let mut updates = match outcome {
            RequestState::Decided(decision) => requests::decided(&offer, decision, true),
            other => vec![Update::Resolve {
                key: offer.key,
                state: other,
            }],
        };
        if state.asks.is_empty() && state.thread.status() == Status::RequiresAction {
            updates.push(Update::Status(Status::Running));
        }
        updates
    };
    shared.apply(updates);
}

/// Reads the agents, models and providers anew after the server said they changed.
fn refresh(shared: &Arc<Shared>) {
    let again = shared.clone();
    let spawned = std::thread::Builder::new()
        .name("opencode-refresh".into())
        .spawn(move || {
            if super::open::learn_offers(&again).is_ok() {
                again.emit(Event::Capabilities);
            }
        });
    if let Err(error) = spawned {
        shared.fail(SessionError::Start(error.to_string()));
    }
}
