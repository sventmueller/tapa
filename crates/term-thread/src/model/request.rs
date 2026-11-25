//! What the agent asks the user: permissions, questions and plans to approve.

use serde_json::Value;

/// Where a request to the user stands.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestState {
    /// It waits for the user.
    Pending,
    /// The user decided.
    Decided(Decision),
    /// The agent no longer needs an answer.
    Withdrawn,
}

/// What the user decided on a request.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The tool may run, once or, with a stored rule, from now on.
    Allowed {
        /// Whether a rule was stored.
        always: bool,
        /// Whether the user changed the input before allowing it.
        edited: bool,
    },
    /// The tool may not run.
    Denied {
        /// What the agent is told.
        message: Option<String>,
    },
    /// The questions were answered.
    Answered(Vec<Answer>),
    /// The plan was approved.
    Approved,
    /// The plan was not accepted.
    Rejected {
        /// What the user wants changed.
        feedback: Option<String>,
        /// Whether the user rejected the plan outright, which ends the turn, rather than asking for another.
        stop: bool,
    },
}

/// The answer to one question.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Answer {
    pub question: String,
    /// The chosen labels, or the user's own words.
    pub answer: String,
}

/// The agent asks to use a tool.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    /// The agent's id for the request, which the answer names.
    pub request_id: String,
    /// The tool call it is about.
    pub tool_call: Option<String>,
    /// The tool's name.
    pub tool_name: String,
    /// The agent's headline for the request.
    pub title: Option<String>,
    /// What the agent says the call does.
    pub description: Option<String>,
    /// Why the agent asks instead of deciding itself.
    pub reason: Option<String>,
    /// The path that made the agent ask, when a path did.
    pub blocked_path: Option<String>,
    /// The input the tool would run with, which the user may change before allowing it.
    pub input: Value,
    /// What the user can answer.
    pub options: Vec<PermissionOption>,
    /// Whether no option may be preselected as allowing.
    pub default_to_no: bool,
    /// Where it stands.
    pub state: RequestState,
}

/// One answer the user can give to a permission request.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionOption {
    /// Allow this call.
    AllowOnce,
    /// Allow this call and store a rule so that similar calls need no question.
    AllowAlways {
        /// The agent's suggestion this option stands for.
        suggestion: usize,
        /// What the rule allows, as the user reads it.
        rule: String,
        /// Where the rule is stored.
        scope: RuleScope,
    },
    /// Refuse this call.
    Deny,
}

/// Where a stored rule applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleScope {
    /// Until the session ends.
    Session,
    /// The project, for this user only.
    ProjectLocal,
    /// The project, for everyone who works on it.
    Project,
    /// Every project of this user.
    User,
    /// A place the agent names in its own words, empty when it names none.
    Other(String),
}

/// The agent asks the user questions.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    /// The agent's id for the request, once the agent waits for the answer.
    pub request_id: Option<String>,
    /// The tool call that asks.
    pub tool_call: String,
    /// The questions.
    pub items: Vec<QuestionItem>,
    /// Where it stands.
    pub state: RequestState,
}

/// One question with its options; the user may also answer in their own words.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct QuestionItem {
    /// A short label for the question.
    pub header: Option<String>,
    pub question: String,
    /// The options offered.
    pub options: Vec<QuestionOption>,
    /// Whether several options may be chosen.
    pub multi_select: bool,
}

/// One option of a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    /// What the option says.
    pub label: String,
    /// What choosing it means.
    pub description: Option<String>,
}

/// The agent asks to leave plan mode with this plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanApproval {
    /// The agent's id for the request, once the agent waits for the answer.
    pub request_id: Option<String>,
    /// The tool call that asks.
    pub tool_call: String,
    /// The plan as markdown.
    pub plan: String,
    /// Where it stands.
    pub state: RequestState,
}
