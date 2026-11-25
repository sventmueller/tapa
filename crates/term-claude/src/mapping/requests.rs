//! The CLI's requests to the user: permissions, questions and plans to approve.

use std::time::SystemTime;

use serde_json::Value;
use term_thread::{
    Decision, EntryKind, Lane, PermissionOption, PermissionRequest, PlanApproval, Question, QuestionItem,
    QuestionOption, RequestState, Status, ToolPatch, ToolStatus, Update,
};

use super::{Ask, Mapper, put};
use crate::control::{PermissionAsk, Suggestion};
use crate::tools::{self, Shape};

fn questions(input: &Value) -> Vec<QuestionItem> {
    let text = |v: &Value, f: &str| v.get(f).and_then(Value::as_str).map(str::to_string);
    input
        .get("questions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|q| {
                    Some(QuestionItem {
                        header: text(q, "header"),
                        question: text(q, "question")?,
                        options: q
                            .get("options")
                            .and_then(Value::as_array)
                            .map(|options| {
                                options
                                    .iter()
                                    .filter_map(|o| {
                                        Some(QuestionOption {
                                            label: text(o, "label")?,
                                            description: text(o, "description"),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        multi_select: q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Mapper {
    /// The updates for a permission request of the CLI.
    pub(crate) fn permission(&mut self, request_id: &str, ask: &PermissionAsk, now: SystemTime) -> Vec<Update> {
        let lane = ask
            .agent_id
            .as_ref()
            .and_then(|agent| self.tasks.get(agent))
            .and_then(|task| task.subagent.clone())
            .map(Lane::Subagent)
            .or_else(|| self.tools.get(&ask.tool_use_id).map(|t| t.lane.clone()))
            .unwrap_or(Lane::Main);
        let call = ask.tool_use_id.clone();
        let mut out = Vec::new();
        let key = match tools::shape(&ask.tool_name) {
            Shape::Question => {
                self.tool_requests.insert(call.clone(), request_id.to_string());
                out.push(put(
                    &lane,
                    Some(call.clone()),
                    now,
                    question(&call, &ask.input, Some(request_id)),
                ));
                call.clone()
            }
            Shape::Plan => {
                self.tool_requests.insert(call.clone(), request_id.to_string());
                out.push(put(
                    &lane,
                    Some(call.clone()),
                    now,
                    plan(&call, &ask.input, Some(request_id)),
                ));
                call.clone()
            }
            Shape::Card | Shape::Todos => {
                // A subagent's request goes to the end of the main timeline: in the subagent's lane it would show in
                // the card the subagent started in, which has often scrolled out of sight by then.
                let key = format!("perm:{request_id}");
                out.push(put(
                    &Lane::Main,
                    Some(key.clone()),
                    now,
                    permission_request(request_id, ask),
                ));
                out.push(Update::Tool {
                    key: call.clone(),
                    patch: ToolPatch {
                        status: Some(ToolStatus::Pending),
                        ..ToolPatch::default()
                    },
                });
                key
            }
        };
        self.asks.insert(request_id.to_string(), Ask { key, tool_use_id: call });
        out.push(self.status(Status::RequiresAction));
        out
    }

    /// The updates for the user's decision on the CLI's request `request_id`.
    pub(crate) fn decided(&mut self, request_id: &str, decision: Decision) -> Vec<Update> {
        let Some(ask) = self.asks.remove(request_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if matches!(decision, Decision::Approved | Decision::Rejected { .. }) {
            self.answered.insert(ask.tool_use_id.clone());
        }
        if matches!(
            decision,
            Decision::Allowed { .. } | Decision::Answered(_) | Decision::Approved
        ) {
            out.push(Update::Tool {
                key: ask.tool_use_id.clone(),
                patch: ToolPatch {
                    status: Some(ToolStatus::InProgress),
                    ..ToolPatch::default()
                },
            });
        }
        out.push(Update::Resolve {
            key: ask.key,
            state: RequestState::Decided(decision),
        });
        if self.asks.is_empty() {
            out.push(self.status(Status::Running));
        }
        out
    }

    /// The updates when the CLI withdraws its request `request_id`.
    pub(crate) fn withdrawn(&mut self, request_id: &str) -> Vec<Update> {
        let Some(ask) = self.asks.remove(request_id) else {
            return Vec::new();
        };
        let mut out = vec![Update::Resolve {
            key: ask.key,
            state: RequestState::Withdrawn,
        }];
        if self.asks.is_empty() {
            out.push(self.status(Status::Running));
        }
        out
    }
}

pub(super) fn question(call: &str, input: &Value, request: Option<&str>) -> EntryKind {
    EntryKind::Question(Question {
        request_id: request.map(str::to_string),
        tool_call: call.to_string(),
        items: questions(input),
        state: RequestState::Pending,
    })
}

pub(super) fn plan(call: &str, input: &Value, request: Option<&str>) -> EntryKind {
    EntryKind::PlanApproval(PlanApproval {
        request_id: request.map(str::to_string),
        tool_call: call.to_string(),
        plan: input
            .get("plan")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        state: RequestState::Pending,
    })
}

fn permission_request(request_id: &str, ask: &PermissionAsk) -> EntryKind {
    let destructive = ask.tool_name == "Bash" && term_thread::shell::destructive_input(&ask.input).is_some();
    let mut options = vec![PermissionOption::AllowOnce];
    if !ask.suppress_always_allow_rule && !destructive {
        options.extend(ask.permission_suggestions.iter().enumerate().filter_map(|(n, raw)| {
            let suggestion = Suggestion::read(raw)?;
            Some(PermissionOption::AllowAlways {
                suggestion: n,
                rule: suggestion.rule,
                scope: suggestion.scope,
            })
        }));
    }
    options.push(PermissionOption::Deny);
    EntryKind::PermissionRequest(PermissionRequest {
        request_id: request_id.to_string(),
        tool_call: Some(ask.tool_use_id.clone()).filter(|c| !c.is_empty()),
        tool_name: ask.tool_name.clone(),
        title: ask.title.clone().or_else(|| ask.display_name.clone()),
        description: ask.description.clone(),
        reason: ask.decision_reason.clone(),
        blocked_path: ask.blocked_path.clone(),
        input: ask.input.clone(),
        options,
        default_to_no: ask.default_to_no || destructive,
        state: RequestState::Pending,
    })
}
