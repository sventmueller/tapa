//! Applying a user message only after the writer has accepted it.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use term_thread::{Change, Thread, Update, UserMessage};

use super::{Capabilities, Launch, RemoteControl, SessionError, State, mapper_for};

impl State {
    pub(super) fn new(launch: &Launch, thread: Thread) -> Self {
        Self {
            mapper: mapper_for(launch),
            thread: Arc::new(thread),
            asks: HashMap::new(),
            capabilities: Capabilities::default(),
            context_refused: false,
            remote: RemoteControl::Unavailable,
            remote_epoch: None,
            remote_url: None,
            remote_asked: 0,
        }
    }

    /// The state lock also keeps a fast reply from reaching the mapper before its prompt.
    pub(super) fn submit(
        &mut self,
        message: UserMessage,
        uuid: &str,
        enqueue: impl FnOnce() -> Result<(), SessionError>,
    ) -> Result<Vec<Change>, SessionError> {
        enqueue()?;
        let mut updates: Vec<_> = message
            .waiting_key()
            .map(|key| Update::Remove { key })
            .into_iter()
            .collect();
        if self.thread.title().is_empty() {
            let title = term_thread::title_from(&message.text);
            if !title.is_empty() {
                updates.push(Update::Title(title));
            }
        }
        updates.extend(self.mapper.user_message(message, uuid, SystemTime::now()));
        Ok(Arc::make_mut(&mut self.thread).apply_all(updates))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_thread::{Lane, Status, ThreadId};

    #[test]
    fn a_refused_line_leaves_the_model_idle_and_can_be_retried() {
        let launch = Launch::new("/bin/false", Vec::new(), "/tmp");
        let thread = Thread::new(ThreadId::from("queue-test"), crate::AGENT, "/tmp");
        let mut state = State::new(&launch, thread);
        let message = UserMessage {
            text: "a prompt".into(),
            ..UserMessage::default()
        };

        for error in [SessionError::Congested, SessionError::Closed] {
            assert!(state.submit(message.clone(), "refused", || Err(error)).is_err());
            assert_eq!(state.thread.status(), Status::Idle);
            assert!(state.thread.title().is_empty());
            assert!(state.thread.entries(&Lane::Main).is_empty());
            assert_eq!(state.mapper.prompts(), 0);
        }

        assert!(state.submit(message, "accepted", || Ok(())).is_ok());
        assert_eq!(state.thread.status(), Status::Running);
        assert_eq!(state.thread.entries(&Lane::Main).len(), 1);
        assert_eq!(state.mapper.prompts(), 1);
    }
}
