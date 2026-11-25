//! Messages sent while the tab's Claude session starts: the timeline shows them at once, the queue row does not, and
//! the session replaces each with its own entry as the message goes out.

use std::time::SystemTime;

use term_thread::{EntryKind, Lane, ThreadId, Update, UserMessage, WAITING_ID};

use super::acp::Agent;
use super::tab::AgentTab;

impl AgentTab {
    /// Whether a message sent now waits for the session to start rather than for a turn to end.
    pub(super) fn waits_for_start(&self) -> bool {
        let state = self.state.borrow();
        matches!(self.agent, Agent::Claude) && !state.running() && !state.source().is_live()
    }

    /// Queues `message` for the session that starts and shows it in the timeline at once.
    pub(super) fn wait_for_start(&self, mut message: UserMessage) {
        message.message_id = Some(format!("{WAITING_ID}{}", ThreadId::new()));
        let mut thread = self.snapshot();
        let changes = thread.apply_all(shown(std::slice::from_ref(&message)));
        self.state.borrow_mut().enqueue(message);
        self.absorb(thread, &changes);
    }

    /// Takes the entry of `message` off the timeline again, as the message did not go out.
    pub(super) fn not_sent(&self, message: &UserMessage) {
        let Some(key) = message.waiting_key() else { return };
        let mut thread = self.snapshot();
        let changes = thread.apply(Update::Remove { key });
        self.absorb(thread, &changes);
    }
}

/// The entries that show the waiting messages of `queue`.
pub(super) fn shown(queue: &[UserMessage]) -> Vec<Update> {
    queue
        .iter()
        .filter_map(|message| {
            Some(Update::Put {
                lane: Lane::Main,
                key: Some(message.waiting_key()?),
                at: SystemTime::now(),
                entry: EntryKind::UserMessage(UserMessage {
                    message_id: None,
                    ..message.clone()
                }),
            })
        })
        .collect()
}

/// Takes the entries of the waiting messages of `queue` out again, so that a history read in comes before them.
pub(super) fn hidden(queue: &[UserMessage]) -> Vec<Update> {
    queue
        .iter()
        .filter_map(UserMessage::waiting_key)
        .map(|key| Update::Remove { key })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_thread::Thread;

    fn waiting(text: &str) -> UserMessage {
        UserMessage {
            text: text.into(),
            message_id: Some(format!("{WAITING_ID}{text}")),
            ..UserMessage::default()
        }
    }

    #[test]
    fn waiting_messages_show_without_an_id_and_go_again() {
        let queue = [waiting("a"), UserMessage::default(), waiting("b")];
        let mut thread = Thread::new(ThreadId::new(), "claude", "/tmp");
        thread.apply_all(shown(&queue));
        let main = thread.entries(&Lane::Main);
        assert_eq!(main.len(), 2);
        assert!(
            main.iter()
                .all(|e| matches!(&e.kind, EntryKind::UserMessage(m) if m.message_id.is_none()))
        );
        thread.apply_all(hidden(&queue));
        assert!(thread.entries(&Lane::Main).is_empty());
    }
}
