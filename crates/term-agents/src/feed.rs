//! The fan-out to the views that follow a session: each has a feed of its own, the
//! control events complete and in order, the invalidations coalesced, and a redraw from the snapshot when they pile
//! up. A view that comes later starts with the snapshot.

use std::collections::BTreeMap;
use std::sync::Arc;

use term_thread::{Change, Thread};

use crate::id::{SubscriberId, TurnId};
use crate::inbox::Coalesced;
use crate::rule::{RequestKind, Verdict};
use crate::turn::TurnEnd;

/// A control event of a session, which every subscriber gets in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    /// The session is open and takes prompts.
    Ready,
    /// What the agent offers changed: models, modes, its account.
    Capabilities,
    /// A prompt went out, and its turn runs.
    TurnStarted(TurnId),
    /// A turn ended (C60).
    TurnEnded(TurnEnd),
    /// A request waits for the user, as the rule left it to them.
    Waiting {
        /// The agent's id of the request.
        request: String,
        /// What it asks.
        kind: RequestKind,
    },
    /// The rule answered a request.
    Decided {
        /// The agent's id of the request.
        request: String,
        /// The answer.
        verdict: Verdict,
    },
    /// Handing an answer to the agent failed, in the path's words.
    AnswerFailed {
        /// The agent's id of the request.
        request: String,
        /// Why.
        message: String,
    },
    /// The path's event stream broke off; the thread may lag behind the agent until it is reconciled (C62).
    StreamGap,
    /// Messages, waiting requests and usage were read from the agent again after a gap.
    Resynced,
    /// Reconciling after a gap failed; the thread may lag behind the agent until `Driver::resync` succeeds (C62).
    NeedsPause(String),
    /// A prompt that waited went out, after its checkpoint, or the path refused it.
    Delivered {
        /// The turn it began.
        turn: TurnId,
        /// The agent's id of the message where the path gives one, which rewinding names; or the path's words
        /// when it refused the prompt.
        sent: Result<Option<String>, String>,
        /// The checkpoint taken before it.
        checkpoint: Option<crate::checkpoint::Checkpoint>,
    },
    /// The checkpoint before a turn's prompt, or its end, could not be recorded.
    CheckpointFailed {
        /// The turn.
        turn: TurnId,
        /// Why.
        message: String,
    },
    /// The end of a turn is recorded in the checkpoint taken before it.
    CheckpointFinished {
        /// The turn.
        turn: TurnId,
        /// The commit of the checkpoint before the turn.
        started: crate::checkpoint::CommitId,
        /// The checkpoint with the end recorded, which restoring takes.
        finished: crate::checkpoint::Checkpoint,
    },
    /// The path reports a failure, such as a missing sign-in, in its own words.
    Failed(String),
    /// Claude Code's failure behind the `Failed` just before, typed, such as a refused rewind.
    ClaudeFailed(term_claude::SessionError),
    /// Remote Control changed: Claude's connection, which `Driver::claude` reads.
    RemoteControl,
    /// Codex's failure behind the `Failed` just before, typed, such as a missing sign-in.
    CodexFailed(term_codex::SessionError),
    /// OpenCode's failure behind the `Failed` just before, typed, such as no connected provider.
    OpenCodeFailed(term_opencode::SessionError),
    /// An ACP agent's failure behind the `Failed` just before, typed, such as a sign-in it wants.
    AcpFailed(term_acp::SessionError),
    /// The agent's process ended.
    Ended {
        /// The end of its error output.
        stderr: String,
    },
}

/// What a subscriber gets: the thread as it is, the invalidations since its last batch and the control events in
/// order. `[Change::Cleared]` means redraw everything from `thread`, as the first batch and one after a backlog do.
#[derive(Debug, Clone)]
pub struct Batch {
    /// The thread as of the driver's last flush, shared with the driver and the other subscribers.
    pub thread: Arc<Thread>,
    /// What changed since the subscriber's last batch.
    pub changes: Vec<Change>,
    /// The control events since then.
    pub signals: Vec<Signal>,
}

impl Batch {
    /// Whether the subscriber redraws from the snapshot instead of applying single changes.
    pub fn is_redraw(&self) -> bool {
        self.changes.contains(&Change::Cleared)
    }
}

#[derive(Default)]
struct Feed {
    changes: Coalesced,
    signals: Vec<Signal>,
}

impl Feed {
    fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.signals.is_empty()
    }
}

/// The feeds of one session's subscribers.
#[derive(Default)]
pub(crate) struct Feeds {
    next: u64,
    feeds: BTreeMap<SubscriberId, Feed>,
}

impl Feeds {
    /// A new subscriber, whose first batch is the snapshot.
    pub(crate) fn subscribe(&mut self) -> SubscriberId {
        self.next += 1;
        let id = SubscriberId(self.next);
        self.feeds.insert(
            id,
            Feed {
                changes: Coalesced::cleared(),
                signals: Vec::new(),
            },
        );
        id
    }

    pub(crate) fn unsubscribe(&mut self, id: SubscriberId) -> bool {
        self.feeds.remove(&id).is_some()
    }

    pub(crate) fn len(&self) -> usize {
        self.feeds.len()
    }

    /// Hands `changes` and `signals` to every feed; the subscribers whose feed was empty until now, which the app
    /// tells that a batch waits.
    pub(crate) fn publish(&mut self, changes: &[Change], signals: &[Signal]) -> Vec<SubscriberId> {
        let mut woken = Vec::new();
        for (id, feed) in &mut self.feeds {
            let was_empty = feed.is_empty();
            feed.changes.add(changes.iter().cloned());
            for signal in signals {
                let repeated = *signal == Signal::Capabilities && feed.signals.last() == Some(signal);
                if !repeated {
                    feed.signals.push(signal.clone());
                }
            }
            if was_empty && !feed.is_empty() {
                woken.push(*id);
            }
        }
        woken
    }

    /// What waits for `id`, emptied; `None` for a subscriber that is not there.
    pub(crate) fn take(&mut self, id: SubscriberId, thread: &Arc<Thread>) -> Option<Batch> {
        let feed = self.feeds.get_mut(&id)?;
        Some(Batch {
            thread: Arc::clone(thread),
            changes: feed.changes.take(),
            signals: std::mem::take(&mut feed.signals),
        })
    }
}

#[cfg(test)]
mod tests {
    use term_thread::ThreadId;

    use super::*;

    #[test]
    fn a_late_subscriber_starts_with_a_redraw_and_each_feed_keeps_its_own_backlog() {
        let thread = Arc::new(Thread::new(ThreadId::from("t"), "x", "/w"));
        let mut feeds = Feeds::default();
        let early = feeds.subscribe();
        assert!(feeds.take(early, &thread).unwrap().is_redraw());
        assert_eq!(feeds.publish(&[Change::Usage], &[Signal::Ready]), [early]);
        let late = feeds.subscribe();
        assert!(feeds.publish(&[Change::Todos], &[Signal::Capabilities]).is_empty());
        let first = feeds.take(early, &thread).unwrap();
        assert!(!first.is_redraw());
        assert_eq!(first.signals, [Signal::Ready, Signal::Capabilities]);
        let joined = feeds.take(late, &thread).unwrap();
        assert!(joined.is_redraw());
        assert!(
            Arc::ptr_eq(&first.thread, &joined.thread),
            "batches share the snapshot instead of copying it"
        );
        assert_eq!(joined.signals, [Signal::Capabilities]);
        assert!(feeds.unsubscribe(late));
        assert!(feeds.take(late, &thread).is_none());
        assert_eq!(feeds.len(), 1);
    }
}
