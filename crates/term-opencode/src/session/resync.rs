//! Catching up with the server after a gap in the event stream (docs/phase-13.md, C62): the session's messages, the
//! requests that wait and its totals are read again, and the thread is rebuilt from them as a resumed one is.

use std::sync::Arc;

use serde_json::Value;
use term_thread::Update;

use super::{Reply, Session, SessionError, Shared, now, open, unit};
use crate::http::encode;

impl Session {
    /// Reads the session's messages, its waiting permissions and forms and its `Session.Info` from the server again,
    /// on a thread of its own, and rebuilds the thread from them: a turn whose end the gap swallowed ends, requests
    /// that came meanwhile wait, the totals are the server's. The title stays when the server names none.
    pub fn resync(&self) -> Reply<()> {
        let (tx, reply) = Reply::channel(unit);
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("opencode-resync".into())
            .spawn(move || drop(tx.send(resync(&shared).map(|()| Value::Null))));
        match spawned {
            Ok(_) => reply,
            Err(error) => Reply::failed(SessionError::Start(error.to_string()), unit),
        }
    }
}

fn resync(shared: &Arc<Shared>) -> Result<(), SessionError> {
    let id = shared.state().session_id.clone().ok_or(SessionError::Closed)?;
    let info = open::data(
        shared.request_now("GET", &format!("/api/session/{}", encode(&id)), None)?,
        "session.get",
    )?;
    let messages = open::messages(shared, &id)?;
    let updates = {
        let mut state = shared.state();
        let state = &mut *state;
        let title = info
            .get("title")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map_or_else(|| state.thread.title().to_string(), str::to_string);
        state.asks.clear();
        let mut updates = state.mapper.history(&messages, now());
        if !title.is_empty() {
            updates.push(Update::Title(title));
        }
        updates.push(Update::Session(id));
        updates.extend(state.mapper.resynced(&info, now()));
        updates
    };
    shared.apply(updates);
    open::waiting_requests(shared)
}
