//! The typed end of a turn (C60). Every path ends a turn in `Idle`, failed or interrupted ones too; what tells the
//! endings apart is the notice the path puts last into the turn: an interruption, an error, an error of the process,
//! or OpenCode's notice of a turn its server stopped for another reason than the user, such as its shutdown.

use term_thread::{EntryId, EntryKind, ErrorKind, Lane, Notice, Thread};

use crate::id::TurnId;

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The agent finished its answer.
    Succeeded,
    /// The turn ended with an error, in the path's words.
    Failed(String),
    /// The user, or the app on their behalf, stopped it.
    Cancelled,
    /// The agent's process or connection went away during the turn.
    ConnectionLost,
}

/// The end of one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    /// The turn.
    pub turn: TurnId,
    /// How it ended.
    pub outcome: Outcome,
    /// The checkpoint taken before its prompt (I6), when there is one.
    pub checkpoint: Option<crate::checkpoint::Checkpoint>,
    /// How many subagents and background items still run (E189); the turn is over only once none does.
    pub running_work: usize,
}

/// The newest entry of the main timeline.
pub(crate) fn last_entry(thread: &Thread) -> Option<EntryId> {
    thread.entries(&Lane::Main).iter().map(|e| e.id).max()
}

fn since(thread: &Thread, after: Option<EntryId>) -> impl DoubleEndedIterator<Item = &EntryKind> {
    thread
        .entries(&Lane::Main)
        .iter()
        .filter(move |e| after.is_none_or(|after| e.id > after))
        .map(|e| &e.kind)
}

/// Whether the agent added anything of its own to the main timeline after `after`.
pub(crate) fn answered(thread: &Thread, after: Option<EntryId>) -> bool {
    since(thread, after).any(|kind| !matches!(kind, EntryKind::UserMessage(_)))
}

/// How the turn whose entries follow `after` ended, by the last interruption or error that no output of the agent
/// follows; an error of the process means the connection is gone.
pub(crate) fn outcome(thread: &Thread, after: Option<EntryId>) -> Outcome {
    for kind in since(thread, after).rev() {
        match kind {
            EntryKind::Notice(Notice::Interrupted) => return Outcome::Cancelled,
            EntryKind::Notice(Notice::Error {
                kind: ErrorKind::Process,
                ..
            }) => return Outcome::ConnectionLost,
            EntryKind::Notice(Notice::Error {
                kind: ErrorKind::Compaction,
                ..
            }) => {}
            EntryKind::Notice(Notice::Error { message, .. }) => return Outcome::Failed(message.clone()),
            EntryKind::Notice(Notice::Info { text, .. })
                if text.starts_with(term_opencode::mapping::OTHER_INTERRUPTION) =>
            {
                return Outcome::ConnectionLost;
            }
            EntryKind::Notice(_) => {}
            _ => return Outcome::Succeeded,
        }
    }
    Outcome::Succeeded
}

/// How many subagents and background items of `thread` still run; a background item the agent keeps for itself, such
/// as a monitor, does not count.
pub fn running_work(thread: &Thread) -> usize {
    let subagents = thread.subagents().iter().filter(|s| !s.status.is_final()).count();
    let background = thread
        .background()
        .iter()
        .filter(|b| !b.status.is_final() && !b.ambient)
        .count();
    subagents + background
}

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use term_thread::{AgentMessage, ThreadId, Update, UserMessage};

    use super::*;

    fn put(thread: &mut Thread, key: &str, entry: EntryKind) {
        thread.apply(Update::Put {
            lane: Lane::Main,
            key: Some(key.into()),
            at: SystemTime::UNIX_EPOCH,
            entry,
        });
    }

    fn said(text: &str) -> EntryKind {
        EntryKind::AgentMessage(AgentMessage {
            markdown: text.into(),
            streaming: false,
        })
    }

    fn error(kind: ErrorKind, message: &str) -> EntryKind {
        EntryKind::Notice(Notice::Error {
            kind,
            message: message.into(),
        })
    }

    #[test]
    fn the_last_notice_without_output_after_it_decides() {
        let mut thread = Thread::new(ThreadId::from("t"), "x", "/w");
        put(&mut thread, "u1", EntryKind::UserMessage(UserMessage::default()));
        let first = last_entry(&thread);
        assert!(!answered(&thread, first));
        put(&mut thread, "e1", error(ErrorKind::Api, "overloaded"));
        put(&mut thread, "m1", said("Recovered."));
        assert_eq!(
            outcome(&thread, first),
            Outcome::Succeeded,
            "an error the agent got past"
        );
        assert!(answered(&thread, first));
        put(&mut thread, "e2", error(ErrorKind::Turn, "max turns"));
        put(
            &mut thread,
            "i1",
            EntryKind::Notice(Notice::Info {
                level: term_thread::InfoLevel::Info,
                text: "hint".into(),
            }),
        );
        assert_eq!(outcome(&thread, first), Outcome::Failed("max turns".into()));
        let second = last_entry(&thread);
        put(&mut thread, "u2", EntryKind::UserMessage(UserMessage::default()));
        put(&mut thread, "x2", EntryKind::Notice(Notice::Interrupted));
        assert_eq!(outcome(&thread, second), Outcome::Cancelled);
        let third = last_entry(&thread);
        put(&mut thread, "m3", said("Half"));
        put(&mut thread, "p3", error(ErrorKind::Process, "gone"));
        assert_eq!(outcome(&thread, third), Outcome::ConnectionLost);
        let fourth = last_entry(&thread);
        put(&mut thread, "c4", error(ErrorKind::Compaction, "no room"));
        assert_eq!(
            outcome(&thread, fourth),
            Outcome::Succeeded,
            "a failed compaction does not fail the turn"
        );
    }
}
