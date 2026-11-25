//! What a session the user drives in an agent tab needs: messages into the turn that runs, permission answers with
//! an edited input and rules to keep, Claude Code's own calls, and the played recordings of the self-test.

use serde_json::Value;
use term_thread::{EntryKind, PermissionOption, PermissionRequest, UserMessage};

use super::{AnswerError, Driver, Prompt, SendError};
use crate::backend::SendFailure;

/// Why `always`, the suggestions of `asked` to keep as rules, may not be kept, with `edited` the input the user
/// changed: a destructive git command is never allowed for good (E27, E33), and only offered suggestions are kept.
fn refused_rules(asked: &PermissionRequest, edited: Option<&Value>, always: &[usize]) -> Option<String> {
    if always.is_empty() {
        return None;
    }
    let destructive = asked.tool_name == "Bash"
        && [Some(&asked.input), edited]
            .into_iter()
            .flatten()
            .any(|input| term_thread::shell::destructive_input(input).is_some());
    if destructive {
        return Some("a destructive git command is allowed once at most".into());
    }
    let offered = |wanted: usize| {
        (asked.options.iter())
            .any(|o| matches!(o, PermissionOption::AllowAlways { suggestion, .. } if *suggestion == wanted))
    };
    (always.iter())
        .find(|n| !offered(**n))
        .map(|n| format!("the request offers no rule {n} to keep"))
}

impl Driver {
    /// The Claude session, for what only Claude Code offers, such as its overlays, Remote Control and its usage;
    /// `None` on the other paths.
    pub fn claude(&self) -> Option<&term_claude::Session> {
        self.backend.claude()
    }

    /// Whether a recording plays instead of an agent.
    pub fn is_replay(&self) -> bool {
        self.backend.is_replay()
    }

    /// Whether a view may hand the session messages: it has not ended, its agent has opened it, and an agent runs it,
    /// not a recording.
    pub fn takes_prompts(&self) -> bool {
        !self.is_ended() && self.backend.is_ready() && !self.backend.is_ended() && !self.backend.is_replay()
    }

    /// Hands `message` to the turn that runs, at once, as the user's messages reach a running Claude turn from an
    /// agent tab; the agent's id of the message where the path gives one. A path that is busy takes it as a prompt
    /// that waits for the turn instead, with no id.
    pub fn steer(&mut self, message: UserMessage) -> Result<Option<String>, SendError> {
        if self.is_ended() {
            return Err(SendError::Ended);
        }
        let queued = |driver: &mut Driver, message| {
            let prompt = Prompt {
                message,
                ..Prompt::default()
            };
            driver.send(prompt).map(|_| None)
        };
        match self.backend.send(message.clone(), None) {
            Ok(id) => Ok(id),
            Err(SendFailure::Busy) => queued(self, message),
            Err(SendFailure::Failed(why)) => Err(SendError::Refused(why)),
        }
    }

    /// Answers the permission request `request` as the user does in an AI tab, with the input they edited and the
    /// suggestions to keep as rules, which only Claude takes; keeping a rule for a destructive git command is refused
    /// (E27, E33).
    pub fn answer_permission_as(
        &mut self,
        request: &str,
        answer: term_claude::PermissionAnswer,
    ) -> Result<(), AnswerError> {
        let Some(EntryKind::PermissionRequest(asked)) = self.pending(request) else {
            return Err(AnswerError::NoRequest(request.into()));
        };
        if let term_claude::PermissionAnswer::Allow { updated_input, always } = &answer
            && let Some(why) = refused_rules(&asked, updated_input.as_ref(), always)
        {
            return Err(AnswerError::Path(why));
        }
        self.backend.permission_as(&asked, answer).map_err(AnswerError::Path)
    }
}

#[cfg(any(test, feature = "selftest"))]
impl Driver {
    /// Plays recorded `lines` into the recording the session plays and hands what changed to the subscribers at
    /// once, without a hop to the main thread; what changed, nothing on the other paths.
    pub fn feed(&mut self, lines: &str) -> Vec<term_thread::Change> {
        let Some(changes) = self.backend.feed(lines) else {
            return Vec::new();
        };
        self.publish_now(&changes);
        changes
    }

    /// Answers the plan `request` of a played recording with the frame a session would send, handed to the
    /// subscribers at once; what changed, nothing on the other paths.
    pub fn replay_plan(&mut self, request: &str, reply: crate::PlanReply) -> Vec<term_thread::Change> {
        let Some(changes) = self.backend.replay_plan(request, reply) else {
            return Vec::new();
        };
        self.publish_now(&changes);
        changes
    }

    fn publish_now(&mut self, changes: &[term_thread::Change]) {
        let mut signals = std::mem::take(&mut self.outbox);
        self.thread = self.backend.snapshot();
        self.decide_requests(&mut signals);
        self.feeds.publish(changes, &signals);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use term_thread::{RequestState, RuleScope};

    use super::*;

    fn asked(command: &str) -> PermissionRequest {
        PermissionRequest {
            request_id: "r1".into(),
            tool_call: None,
            tool_name: "Bash".into(),
            title: None,
            description: None,
            reason: None,
            blocked_path: None,
            input: json!({ "command": command }),
            options: vec![
                PermissionOption::AllowOnce,
                PermissionOption::AllowAlways {
                    suggestion: 1,
                    rule: "Bash(cargo test:*)".into(),
                    scope: RuleScope::Project,
                },
                PermissionOption::Deny,
            ],
            default_to_no: false,
            state: RequestState::Pending,
        }
    }

    #[test]
    fn only_offered_rules_are_kept_and_never_one_for_destructive_git() {
        assert_eq!(refused_rules(&asked("cargo test"), None, &[1]), None);
        assert!(refused_rules(&asked("cargo test"), None, &[0]).is_some());
        assert!(refused_rules(&asked("git reset --hard"), None, &[1]).is_some());
        assert_eq!(
            refused_rules(&asked("git reset --hard"), None, &[]),
            None,
            "once is allowed"
        );
        let edited = json!({ "command": "git push --force" });
        assert!(refused_rules(&asked("cargo test"), Some(&edited), &[1]).is_some());
    }
}
