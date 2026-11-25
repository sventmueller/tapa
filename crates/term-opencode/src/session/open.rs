//! Opening a session, in the order the server needs: the version check, the address the server prints, `GET
//! /api/info`, the event stream (before anything that has events), the agents, models and providers, then the
//! session itself, created or resumed with its messages and the requests that still wait.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use serde_json::Value;
use term_thread::{Status, Update};

use super::{SessionError, Shared, events, lock, now};
use crate::http::{Endpoint, encode};
use crate::mapping::Mapper;

/// How long the server may take to print its address.
const URL_TIMEOUT: Duration = Duration::from_secs(30);

/// How many messages one page of a resumed session's history holds.
const PAGE: u32 = 200;

/// The most pages of history read; a longer session shows its start.
const PAGES: usize = 50;

/// Runs the handshake; what goes wrong is reported, and an unsupported release also ends the process.
pub(super) fn open(shared: &Arc<Shared>, url: &Receiver<String>) {
    match handshake(shared, url) {
        Ok(()) => {}
        Err(error @ SessionError::Unsupported { .. }) => {
            shared.fail(error);
            shared.stopping.store(true, Ordering::SeqCst);
            shared.close();
            shared.supervisor.stop(shared.process, Duration::from_millis(500));
            // The end of the server's output can come late: a process started meanwhile may hold its pipe open.
            shared.finish();
        }
        Err(_) if shared.is_ended() => {}
        Err(error) => shared.fail(error),
    }
}

fn handshake(shared: &Arc<Shared>, url: &Receiver<String>) -> Result<(), SessionError> {
    let launch = &shared.launch;
    let version = match launch.version {
        Some(known) => crate::version::check(Some(known))?,
        None => crate::version::probe(&shared.supervisor, &launch.program, &launch.env)?,
    };
    shared.state().caps.version = Some(version);
    let address = url
        .recv_timeout(URL_TIMEOUT)
        .map_err(|_| SessionError::Start("opencode named no address".into()))?;
    let endpoint = Endpoint::new(&address, shared.password.expose()).map_err(SessionError::Http)?;
    *lock(&shared.endpoint) = Some(endpoint);
    let info = shared.request_now("GET", "/api/info", None)?;
    shared.state().caps.learn_info(&info);
    events::subscribe(shared)?;
    learn_offers(shared)?;
    open_session(shared)?;
    if launch.resume.is_some() {
        waiting_requests(shared)?;
    }
    shared.emit(super::Event::Capabilities);
    if !shared.state().caps.signed_in() {
        shared.fail(SessionError::SignIn);
    }
    Ok(())
}

/// The agents, models, default model and providers of the project folder.
pub(super) fn learn_offers(shared: &Shared) -> Result<(), SessionError> {
    let location = shared.location();
    let agents = shared.request_now("GET", &format!("/api/agent?{location}"), None)?;
    let models = shared.request_now("GET", &format!("/api/model?{location}"), None)?;
    let default = shared.request_now("GET", &format!("/api/model/default?{location}"), None)?;
    let providers = shared.request_now("GET", &format!("/api/provider?{location}"), None)?;
    let mut state = shared.state();
    state.caps.learn_agents(&agents);
    state.caps.learn_models(&models);
    state.caps.learn_default(&default);
    state.caps.learn_providers(&providers);
    Ok(())
}

pub(super) fn data(result: Value, what: &str) -> Result<Value, SessionError> {
    result
        .get("data")
        .filter(|d| d.get("id").is_some())
        .cloned()
        .ok_or_else(|| SessionError::Malformed(format!("{what} named no session")))
}

/// Every message of the session `id`, oldest first.
pub(super) fn messages(shared: &Shared, id: &str) -> Result<Vec<Value>, SessionError> {
    let base = format!("/api/session/{}/message", encode(id));
    let mut target = format!("{base}?order=asc&limit={PAGE}");
    let mut all = Vec::new();
    for _ in 0..PAGES {
        let mut page = shared.request_now("GET", &target, None)?;
        if let Some(Value::Array(messages)) = page.get_mut("data").map(Value::take) {
            all.extend(messages);
        }
        match page.get("cursor").and_then(|c| c.get("next")).and_then(Value::as_str) {
            Some(cursor) => target = format!("{base}?cursor={}&limit={PAGE}", encode(cursor)),
            None => break,
        }
    }
    Ok(all)
}

fn open_session(shared: &Arc<Shared>) -> Result<(), SessionError> {
    let launch = &shared.launch;
    let (info, history) = match &launch.resume {
        Some(id) => {
            let info = data(
                shared.request_now("GET", &format!("/api/session/{}", encode(id)), None)?,
                "session.get",
            )?;
            (info, Some(messages(shared, id)?))
        }
        None => {
            let body = launch.create_body();
            (
                data(
                    shared.request_now("POST", "/api/session", Some(&body))?,
                    "session.create",
                )?,
                None,
            )
        }
    };
    let id = info.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let updates = {
        let mut state = shared.state();
        let state = &mut *state;
        state.caps.learn_session(&info);
        state.mapper.set_session(&id);
        let mut updates = Vec::new();
        if let Some(messages) = &history {
            updates.extend(state.mapper.history(messages, now()));
            if let Some(title) = info.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                updates.push(Update::Title(title.to_string()));
            }
        }
        updates.extend(header(&mut state.mapper, &state.caps, &id));
        state.session_id = Some(id);
        updates
    };
    shared.apply(updates);
    Ok(())
}

/// The thread's head as the opened session sets it.
fn header(mapper: &mut Mapper, caps: &crate::Capabilities, id: &str) -> Vec<Update> {
    let mut updates = mapper.settle(now());
    updates.push(Update::Session(id.to_string()));
    if let Some(model) = caps.effective_model() {
        updates.push(Update::Model(model.id()));
    }
    updates.push(Update::Effort(caps.variant().map(str::to_string)));
    if let Some(mode) = caps.mode() {
        updates.push(Update::Mode(mode.to_string()));
    }
    updates.push(Update::Status(Status::Idle));
    updates
}

/// The permissions and forms of a resumed session that wait for an answer.
pub(super) fn waiting_requests(shared: &Arc<Shared>) -> Result<(), SessionError> {
    let target = format!("/api/permission/request?{}", shared.location());
    let permissions = shared.request_now("GET", &target, None)?;
    let forms = shared.request_now("GET", &shared.session_path("/form")?, None)?;
    let own = shared.state().session_id.clone();
    let mut updates = Vec::new();
    {
        let mut state = shared.state();
        for permission in permissions.get("data").and_then(Value::as_array).into_iter().flatten() {
            if permission.get("sessionID").and_then(Value::as_str) != own.as_deref() {
                continue;
            }
            if let Some((id, offer, made)) = state.mapper.permission(permission, now()) {
                state.asks.insert(id, offer);
                updates.extend(made);
            }
        }
        for form in forms.get("data").and_then(Value::as_array).into_iter().flatten() {
            if let Some((id, offer, made)) = state.mapper.form(form, now()) {
                state.asks.insert(id, offer);
                updates.extend(made);
            }
        }
    }
    shared.apply(updates);
    Ok(())
}
