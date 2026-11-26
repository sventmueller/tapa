//! Transport and event routing for sessions attached to the shared Codex daemon.

use std::io::BufRead;

use serde_json::{Value, json};

use super::{After, Event, Reply, Session, SessionError, Shared};
use crate::protocol::{method, text};
use crate::{RemoteControl, wire::Message, ws};

impl Shared {
    pub(super) fn read_websocket(
        self: &std::sync::Arc<Self>,
        reader: &mut impl BufRead,
        upgraded: std::sync::mpsc::Sender<()>,
    ) {
        if let Err(error) = ws::read_upgrade(reader) {
            if !self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                self.fail(SessionError::Start(error.to_string()));
            }
            return;
        }
        let _ = upgraded.send(());
        let mut pong = |opcode, payload: Vec<u8>| {
            if opcode == ws::PING {
                let _ = self.send_bytes(ws::frame(ws::PONG, &payload, true));
            }
        };
        while let Ok(Some(text)) = ws::read_message(reader, ws::Peer::Server, &mut pong) {
            if let Some(message) = Message::parse(&text) {
                self.dispatch(message);
            }
        }
    }

    pub(super) fn owns_thread(&self, id: &str) -> bool {
        let state = self.state();
        state.thread_id.as_deref().or(self.launch.resume.as_deref()) == Some(id)
            || state.shared_children.contains(id)
            || state.mapper.knows_task_thread(id)
    }

    /// Keeps a tab's transcript and approvals confined to its own thread and descendants.
    pub(super) fn shared_notification(&self, method: &str, params: &Value) -> bool {
        if method == "remoteControl/status/changed" {
            self.state().remote = RemoteControl::read(params);
            self.emit(Event::RemoteControl);
            return false;
        }
        if method == "thread/started" {
            let thread = &params["thread"];
            let Some(id) = text(thread, "id") else { return false };
            if text(thread, "parentThreadId").is_some_and(|parent| self.owns_thread(parent)) {
                self.state().shared_children.insert(id.to_string());
            }
            return self.owns_thread(id);
        }
        text(params, "threadId").is_none_or(|thread| self.owns_thread(thread))
    }
}

impl Session {
    /// The last Remote Control status reported by the shared daemon, or off on a private server.
    pub fn remote_control(&self) -> RemoteControl {
        self.shared.state().remote.clone()
    }

    /// Disables Remote Control for all of the daemon's clients and saves that choice.
    pub fn disable_remote_control(&self) -> Result<Reply<RemoteControl>, SessionError> {
        if !self.shared.launch.shared || self.is_ended() {
            return Err(SessionError::Closed);
        }
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                shared.state().remote = RemoteControl::read(result);
                shared.emit(Event::RemoteControl);
            }
        });
        Ok(self
            .shared
            .call(method::REMOTE_CONTROL_DISABLE, json!({}), Some(after), |result| {
                Ok(RemoteControl::read(&result))
            }))
    }
}
