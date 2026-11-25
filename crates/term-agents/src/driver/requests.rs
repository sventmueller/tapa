//! The requests of the agent: the rule decides each once, and what it leaves to the user, the user answers through
//! the driver, also without an AI tab (F72).

use term_thread::{Answer, EntryKind};

use super::Driver;
use crate::backend::PlanReply;
use crate::feed::Signal;
use crate::rule::{Request, Verdict};

/// Why an answer did not reach the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    /// No request with this id waits for this kind of answer.
    NoRequest(String),
    /// The path refused it or cannot take it, in its own words.
    Path(String),
}

impl std::fmt::Display for AnswerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnswerError::NoRequest(id) => write!(f, "no request {id} waits for this answer"),
            AnswerError::Path(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for AnswerError {}

/// The agent's id of a request that waits, once it has one.
fn request_id(kind: &EntryKind) -> Option<String> {
    match kind {
        EntryKind::PermissionRequest(r) => Some(r.request_id.clone()),
        EntryKind::Question(q) => q.request_id.clone(),
        EntryKind::PlanApproval(p) => p.request_id.clone(),
        _ => None,
    }
}

impl Driver {
    /// Hands each request that came since the last flush to the rule, and carries out what it says.
    pub(super) fn decide_requests(&mut self, signals: &mut Vec<Signal>) {
        let waiting: Vec<(String, EntryKind)> = self
            .thread
            .pending_requests()
            .into_iter()
            .filter_map(|entry| request_id(&entry.kind).map(|id| (id, entry.kind.clone())))
            .collect();
        for (id, kind) in waiting {
            if !self.handled.insert(id.clone()) {
                continue;
            }
            let request = match &kind {
                EntryKind::PermissionRequest(r) => Request::Permission(r),
                EntryKind::Question(q) => Request::Question(q),
                EntryKind::PlanApproval(p) => Request::Plan(p),
                _ => continue,
            };
            let verdict = self.rule.decide(&request);
            let answered = match (&request, &verdict) {
                (_, Verdict::User) | (Request::Question(_), Verdict::Allow) => None,
                (Request::Permission(r), Verdict::Allow) => Some(self.backend.permission(r, true, None)),
                (Request::Permission(r), Verdict::Deny(why)) => Some(self.backend.permission(r, false, Some(why))),
                (Request::Question(q), Verdict::Deny(why)) => {
                    let answers = q
                        .items
                        .iter()
                        .map(|item| Answer {
                            question: item.question.clone(),
                            answer: why.clone(),
                        })
                        .collect();
                    Some(self.backend.questions(&id, answers))
                }
                (Request::Plan(_), Verdict::Allow) => Some(self.backend.plan(&id, PlanReply::Approve)),
                (Request::Plan(_), Verdict::Deny(why)) => {
                    Some(self.backend.plan(&id, PlanReply::Revise(Some(why.clone()))))
                }
            };
            let kind = request.kind();
            match answered {
                Some(Ok(())) => signals.push(Signal::Decided { request: id, verdict }),
                Some(Err(message)) => {
                    signals.push(Signal::AnswerFailed {
                        request: id.clone(),
                        message,
                    });
                    self.left.insert(id.clone());
                    signals.push(Signal::Waiting { request: id, kind });
                }
                None => {
                    self.left.insert(id.clone());
                    signals.push(Signal::Waiting { request: id, kind });
                }
            }
        }
    }

    /// Whether a request the rule left to the user waits for them.
    pub fn waits_for_user(&self) -> bool {
        (self.thread.pending_requests().iter()).any(|e| request_id(&e.kind).is_some_and(|id| self.left.contains(&id)))
    }

    /// The request `request`, as it waits now.
    pub(super) fn pending(&self, request: &str) -> Option<EntryKind> {
        let thread = self.backend.snapshot();
        thread
            .pending_requests()
            .into_iter()
            .find(|e| request_id(&e.kind).as_deref() == Some(request))
            .map(|e| e.kind.clone())
    }

    /// Allows the permission request `request` once, or refuses it, telling the agent `reason` where the path can.
    pub fn answer_permission(&mut self, request: &str, allow: bool, reason: Option<&str>) -> Result<(), AnswerError> {
        let Some(EntryKind::PermissionRequest(asked)) = self.pending(request) else {
            return Err(AnswerError::NoRequest(request.into()));
        };
        self.backend
            .permission(&asked, allow, reason)
            .map_err(AnswerError::Path)
    }

    /// Answers the questions of `request`.
    pub fn answer_questions(&mut self, request: &str, answers: Vec<Answer>) -> Result<(), AnswerError> {
        let Some(EntryKind::Question(_)) = self.pending(request) else {
            return Err(AnswerError::NoRequest(request.into()));
        };
        self.backend.questions(request, answers).map_err(AnswerError::Path)
    }

    /// Answers the plan `request`.
    pub fn answer_plan(&mut self, request: &str, reply: PlanReply) -> Result<(), AnswerError> {
        let Some(EntryKind::PlanApproval(_)) = self.pending(request) else {
            return Err(AnswerError::NoRequest(request.into()));
        };
        self.backend.plan(request, reply).map_err(AnswerError::Path)
    }
}
