//! Who answers what the agent asks: the rule a driver is handed decides each permission request, question and plan
//! as it comes, or leaves it to the user.

use term_thread::{PermissionRequest, PlanApproval, Question};

/// A request of the agent that waits for an answer.
#[derive(Debug, Clone, Copy)]
pub enum Request<'a> {
    /// It asks to use a tool.
    Permission(&'a PermissionRequest),
    /// It asks the user questions.
    Question(&'a Question),
    /// It asks to carry out a plan.
    Plan(&'a PlanApproval),
}

/// What sort of request waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    /// A permission request.
    Permission,
    /// Questions.
    Question,
    /// A plan.
    Plan,
}

impl Request<'_> {
    /// What sort of request it is.
    pub fn kind(&self) -> RequestKind {
        match self {
            Request::Permission(_) => RequestKind::Permission,
            Request::Question(_) => RequestKind::Question,
            Request::Plan(_) => RequestKind::Plan,
        }
    }
}

/// What the rule says to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Allow the tool once, approve the plan; for questions the same as `User`.
    Allow,
    /// Refuse it and tell the agent this, where the path carries a reason: a denied tool, a plan sent back, or the
    /// answer to every question.
    Deny(String),
    /// Leave it to the user, who answers through the driver.
    User,
}

/// Decides the requests of one session; it runs on the main thread, as the driver does.
pub trait RequestRule {
    /// What to do about `request`, which the driver hands over once.
    fn decide(&self, request: &Request<'_>) -> Verdict;
}

/// Everything goes to the user, as in an AI tab today.
#[derive(Debug, Clone, Copy, Default)]
pub struct UserRule;

impl RequestRule for UserRule {
    fn decide(&self, _: &Request<'_>) -> Verdict {
        Verdict::User
    }
}

#[cfg(test)]
mod tests {
    use term_thread::RequestState;

    use super::*;

    #[test]
    fn the_users_rule_leaves_everything_to_the_user() {
        let plan = PlanApproval {
            request_id: Some("p1".into()),
            tool_call: "c2".into(),
            plan: "Do it".into(),
            state: RequestState::Pending,
        };
        assert_eq!(UserRule.decide(&Request::Plan(&plan)), Verdict::User);
        assert_eq!(Request::Plan(&plan).kind(), RequestKind::Plan);
    }
}
