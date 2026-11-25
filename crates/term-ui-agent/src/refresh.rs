//! Which parts of the app observe a change; streaming text only invalidates its timeline row.

use term_thread::{Change, EntryKind, Field, Lane, Thread};

#[derive(Default)]
pub(super) struct Refresh {
    pub(crate) controls: bool,
    pub(crate) status: bool,
    pub(crate) plans: bool,
    pub(crate) overview: bool,
    /// Whether the agents panel builds its session list again; a new title arrives with the write of the index.
    pub(crate) sessions: bool,
}

impl Refresh {
    pub(crate) fn of(thread: &Thread, changes: &[Change]) -> Refresh {
        let mut refresh = Refresh::default();
        for change in changes {
            match change {
                Change::Cleared => {
                    return Refresh {
                        controls: true,
                        status: true,
                        plans: true,
                        overview: true,
                        sessions: true,
                    };
                }
                Change::Header(field) => {
                    refresh.controls |= *field != Field::Activity;
                    refresh.status = true;
                    refresh.overview = true;
                    refresh.sessions |= *field == Field::Session;
                }
                Change::Usage => {
                    refresh.status = true;
                    refresh.overview = true;
                }
                Change::Subagent(_) | Change::Background => refresh.overview = true,
                Change::Inserted {
                    lane: Lane::Main, id, ..
                }
                | Change::Updated { lane: Lane::Main, id } => match thread.entry(*id).map(|entry| &entry.kind) {
                    Some(EntryKind::Notice(_)) => refresh.status = true,
                    Some(EntryKind::UserMessage(_)) => {
                        refresh.status = true;
                        refresh.overview = true;
                    }
                    Some(EntryKind::PlanApproval(_)) => refresh.plans = true,
                    _ => {}
                },
                Change::Removed { lane: Lane::Main, .. } => {
                    refresh.status = true;
                    refresh.plans = true;
                    refresh.overview = true;
                }
                _ => {}
            }
        }
        refresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_thread::{AgentMessage, ThreadId, Update};

    #[test]
    fn streamed_text_does_not_refresh_controls_plans_status_or_other_sessions() {
        let mut thread = Thread::new(ThreadId::from("t"), "test", "/tmp");
        thread.apply(Update::Put {
            lane: Lane::Main,
            key: Some("m".into()),
            at: std::time::UNIX_EPOCH,
            entry: EntryKind::AgentMessage(AgentMessage {
                markdown: "hello".into(),
                streaming: true,
            }),
        });
        let changes = thread.apply(Update::Append {
            key: "m".into(),
            text: " world".into(),
        });
        let refresh = Refresh::of(&thread, &changes);
        assert!(!refresh.controls && !refresh.status && !refresh.plans && !refresh.overview && !refresh.sessions);
        let refresh = Refresh::of(&thread, &[Change::Header(Field::Model)]);
        assert!(refresh.controls && !refresh.sessions);
        let refresh = Refresh::of(&thread, &[Change::Usage]);
        assert!(refresh.status && refresh.overview && !refresh.controls && !refresh.sessions);
        let refresh = Refresh::of(&thread, &[Change::Header(Field::Activity)]);
        assert!(refresh.overview && !refresh.sessions);
        let refresh = Refresh::of(&thread, &[Change::Header(Field::Session)]);
        assert!(refresh.sessions);
    }
}
