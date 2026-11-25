//! What the server asks the user: commands, file changes and permissions to approve, questions, and the input an
//! MCP server wants. Each becomes an entry of the thread and an `Ask` that knows how the answer goes on the wire.

use std::time::SystemTime;

use serde_json::{Value, json};
use term_thread::{
    Decision, EntryKind, PermissionOption, PermissionRequest, Question, QuestionItem, QuestionOption, RequestState,
    Status, ToolPatch, ToolStatus, Update,
};

use crate::mapping::{Mapper, put};
use crate::protocol::{method, text};

/// One answer the user can pick, in the order the entry offers them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pick {
    /// `accept`.
    Accept,
    /// `acceptForSession`.
    AcceptForSession,
    /// `acceptWithExecpolicyAmendment` with the rule the server proposed.
    Amend(Vec<String>),
    /// `decline`: refused, the turn goes on.
    Decline,
    /// The requested permissions, for the turn or the session.
    Grant {
        /// Whether for the rest of the session.
        session: bool,
    },
}

impl Pick {
    fn option(&self, n: usize, rule: &str) -> PermissionOption {
        match self {
            Pick::Accept | Pick::Grant { session: false } => PermissionOption::AllowOnce,
            Pick::AcceptForSession | Pick::Grant { session: true } => PermissionOption::AllowAlways {
                suggestion: n,
                rule: rule.to_string(),
                scope: term_thread::RuleScope::Session,
            },
            Pick::Amend(words) => PermissionOption::AllowAlways {
                suggestion: n,
                rule: words.join(" "),
                scope: term_thread::RuleScope::User,
            },
            Pick::Decline => PermissionOption::Deny,
        }
    }

    /// What the thread records for this answer.
    pub(crate) fn decision(&self) -> Decision {
        match self {
            Pick::Accept | Pick::Grant { session: false } => Decision::Allowed {
                always: false,
                edited: false,
            },
            Pick::AcceptForSession | Pick::Amend(_) | Pick::Grant { session: true } => Decision::Allowed {
                always: true,
                edited: false,
            },
            Pick::Decline => Decision::Denied { message: None },
        }
    }
}

/// What a request is about, which decides the shape of its answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AskKind {
    /// `item/commandExecution/requestApproval`.
    Command,
    /// `item/fileChange/requestApproval`.
    FileChange,
    /// `item/permissions/requestApproval`, with the permissions asked for.
    Permissions(Value),
    /// `item/tool/requestUserInput`, with the ids of its questions by their text.
    Questions(Vec<(String, String)>),
    /// `mcpServer/elicitation/request`.
    Elicitation,
}

/// A request waiting for the user.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Offer {
    pub(crate) kind: AskKind,
    pub(crate) picks: Vec<Pick>,
    /// The thread entry of the request.
    pub(crate) key: String,
    /// The tool card it is about.
    pub(crate) item: Option<String>,
}

/// The answer to send for `pick`, or with `None` for a request the user dismissed or the turn ended.
pub(crate) fn response(kind: &AskKind, pick: Option<&Pick>) -> Value {
    match (kind, pick) {
        (AskKind::Command | AskKind::FileChange, Some(Pick::Amend(words))) => {
            json!({"decision": {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": words}}})
        }
        (AskKind::Command | AskKind::FileChange, Some(pick)) => json!({"decision": match pick {
            Pick::AcceptForSession | Pick::Grant { session: true } => "acceptForSession",
            Pick::Decline => "decline",
            _ => "accept",
        }}),
        (AskKind::Command | AskKind::FileChange, None) => json!({"decision": "cancel"}),
        (AskKind::Permissions(asked), Some(Pick::Grant { session })) => {
            json!({"permissions": asked, "scope": if *session { "session" } else { "turn" }})
        }
        (AskKind::Permissions(_), _) => json!({"permissions": {}, "scope": "turn"}),
        (AskKind::Questions(_), _) => json!({"answers": {}}),
        (AskKind::Elicitation, Some(Pick::Accept)) => json!({"action": "accept", "content": null}),
        (AskKind::Elicitation, Some(_)) => json!({"action": "decline", "content": null}),
        (AskKind::Elicitation, None) => json!({"action": "cancel", "content": null}),
    }
}

/// The picks for a command; none that stores a rule for a git command that destroys work (E27), and only one-time
/// ones for a checkpoint that wants a person's fresh decision.
fn command_picks(params: &Value, destructive: bool) -> Vec<Pick> {
    if destructive || text(params, "kind") == Some("humanCommand") {
        return vec![Pick::Accept, Pick::Decline];
    }
    let mut picks = vec![Pick::Accept, Pick::AcceptForSession];
    if let Some(words) = params
        .get("proposedExecpolicyAmendment")
        .and_then(|a| serde_json::from_value::<Vec<String>>(a.clone()).ok())
        .filter(|w| !w.is_empty())
    {
        picks.push(Pick::Amend(words));
    }
    picks.push(Pick::Decline);
    picks
}

/// The questions of `item/tool/requestUserInput`, and their ids by their text.
fn questions(params: &Value) -> (Vec<QuestionItem>, Vec<(String, String)>) {
    let mut items = Vec::new();
    let mut ids = Vec::new();
    for q in params.get("questions").and_then(Value::as_array).into_iter().flatten() {
        let (Some(id), Some(question)) = (text(q, "id"), text(q, "question")) else {
            continue;
        };
        let options = q
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|o| {
                Some(QuestionOption {
                    label: text(o, "label")?.to_string(),
                    description: text(o, "description").map(str::to_string),
                })
            })
            .collect();
        items.push(QuestionItem {
            header: text(q, "header").map(str::to_string),
            question: question.to_string(),
            options,
            multi_select: false,
        });
        ids.push((question.to_string(), id.to_string()));
    }
    (items, ids)
}

impl Mapper {
    fn item_of(params: &Value) -> Option<String> {
        Some(format!("item:{}:{}", text(params, "turnId")?, text(params, "itemId")?))
    }

    /// The entry and offer for the server's request `method` with `params`; `ask` is the app's id for it, which the
    /// answer names. `None` for a request this crate does not take.
    pub(crate) fn request(
        &mut self,
        ask: &str,
        method: &str,
        params: &Value,
        at: SystemTime,
    ) -> Option<(Offer, Vec<Update>)> {
        let item = Self::item_of(params);
        let reason = text(params, "reason").map(str::to_string);
        let (kind, picks, entry) = match method {
            method::COMMAND_APPROVAL => {
                let command = text(params, "command").unwrap_or_default();
                let destructive = term_thread::shell::destructive_git(command).is_some();
                let picks = command_picks(params, destructive);
                let entry = permission(ask, &item, "Bash", command, &picks, params, destructive);
                (AskKind::Command, picks, entry)
            }
            method::FILE_CHANGE_APPROVAL => {
                let picks = vec![Pick::Accept, Pick::AcceptForSession, Pick::Decline];
                let mut entry = permission(ask, &item, "Edit", "", &picks, params, false);
                entry.blocked_path = text(params, "grantRoot").map(str::to_string);
                (AskKind::FileChange, picks, entry)
            }
            method::PERMISSIONS_APPROVAL => {
                let asked = params.get("permissions").cloned().unwrap_or_else(|| json!({}));
                let picks = vec![
                    Pick::Grant { session: false },
                    Pick::Grant { session: true },
                    Pick::Decline,
                ];
                let entry = permission(ask, &item, "permissions", "", &picks, params, false);
                (AskKind::Permissions(asked), picks, entry)
            }
            method::ELICITATION => {
                let picks = vec![Pick::Accept, Pick::Decline];
                let server = text(params, "serverName").unwrap_or_default();
                let mut entry = permission(ask, &None, server, "", &picks, params, false);
                entry.title = text(params, "message").map(str::to_string);
                entry.description = text(params, "url").map(str::to_string);
                (AskKind::Elicitation, picks, entry)
            }
            method::USER_INPUT => return Some(self.question(ask, item, params, at)),
            _ => return None,
        };
        let key = format!("permission-{ask}");
        let entry = PermissionRequest { reason, ..entry };
        let mut out = Vec::new();
        if let Some(item) = &item {
            out.push(Update::Tool {
                key: item.clone(),
                patch: ToolPatch {
                    status: Some(ToolStatus::Pending),
                    ..ToolPatch::default()
                },
            });
        }
        out.push(put(key.clone(), at, EntryKind::PermissionRequest(entry)));
        out.push(Update::Status(Status::RequiresAction));
        Some((Offer { kind, picks, key, item }, out))
    }

    fn question(&mut self, ask: &str, item: Option<String>, params: &Value, at: SystemTime) -> (Offer, Vec<Update>) {
        let (items, ids) = questions(params);
        let key = format!("question-{ask}");
        let entry = Question {
            request_id: Some(ask.to_string()),
            tool_call: item.clone().unwrap_or_default(),
            items,
            state: RequestState::Pending,
        };
        let out = vec![
            put(key.clone(), at, EntryKind::Question(entry)),
            Update::Ask {
                key: key.clone(),
                request_id: ask.to_string(),
            },
            Update::Status(Status::RequiresAction),
        ];
        let offer = Offer {
            kind: AskKind::Questions(ids),
            picks: Vec::new(),
            key,
            item,
        };
        (offer, out)
    }
}

fn permission(
    ask: &str,
    item: &Option<String>,
    tool: &str,
    command: &str,
    picks: &[Pick],
    params: &Value,
    destructive: bool,
) -> PermissionRequest {
    let rule = if command.is_empty() { tool } else { command };
    PermissionRequest {
        request_id: ask.to_string(),
        tool_call: item.clone(),
        tool_name: tool.to_string(),
        title: Some(command.to_string()).filter(|c| !c.is_empty()),
        description: None,
        reason: None,
        blocked_path: None,
        input: params.clone(),
        options: picks.iter().enumerate().map(|(n, p)| p.option(n, rule)).collect(),
        default_to_no: destructive,
        state: RequestState::Pending,
    }
}

/// The updates once the user answered the request behind `offer` with `decision`; `still_asking` says whether
/// other requests wait.
pub(crate) fn decided(offer: &Offer, decision: Decision, still_asking: bool) -> Vec<Update> {
    let allowed = matches!(decision, Decision::Allowed { .. } | Decision::Answered(_));
    let mut out = Vec::new();
    if let Some(item) = offer.item.as_ref().filter(|_| offer.kind != AskKind::Elicitation) {
        out.push(Update::Tool {
            key: item.clone(),
            patch: ToolPatch {
                status: Some(if allowed {
                    ToolStatus::InProgress
                } else {
                    ToolStatus::Failed
                }),
                ..ToolPatch::default()
            },
        });
    }
    out.push(Update::Resolve {
        key: offer.key.clone(),
        state: RequestState::Decided(decision),
    });
    if !still_asking {
        out.push(Update::Status(Status::Running));
    }
    out
}

/// The answers to questions in the protocol's shape: each question's id with the user's answer.
pub(crate) fn answers(ids: &[(String, String)], given: &[term_thread::Answer]) -> Value {
    let mut map = serde_json::Map::new();
    for answer in given {
        if let Some((_, id)) = ids.iter().find(|(q, _)| *q == answer.question) {
            map.insert(id.clone(), json!({"answers": [answer.answer]}));
        }
    }
    json!({"answers": map})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pick_answers_with_the_decision_the_protocol_names() {
        let answer = |kind: &AskKind, pick: Option<Pick>| response(kind, pick.as_ref());
        assert_eq!(
            answer(&AskKind::Command, Some(Pick::Accept)),
            json!({"decision": "accept"})
        );
        assert_eq!(
            answer(&AskKind::FileChange, Some(Pick::AcceptForSession)),
            json!({"decision": "acceptForSession"})
        );
        assert_eq!(
            answer(&AskKind::Command, Some(Pick::Decline)),
            json!({"decision": "decline"})
        );
        assert_eq!(answer(&AskKind::Command, None), json!({"decision": "cancel"}));
        assert_eq!(
            answer(
                &AskKind::Command,
                Some(Pick::Amend(vec!["cargo".into(), "test".into()]))
            ),
            json!({"decision": {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["cargo", "test"]}}})
        );
        let asked = json!({"network": {"enabled": true}});
        let kind = AskKind::Permissions(asked.clone());
        assert_eq!(
            answer(&kind, Some(Pick::Grant { session: true })),
            json!({"permissions": asked, "scope": "session"})
        );
        assert_eq!(
            answer(&kind, Some(Pick::Decline)),
            json!({"permissions": {}, "scope": "turn"})
        );
        assert_eq!(
            answer(&AskKind::Elicitation, None),
            json!({"action": "cancel", "content": null})
        );
    }

    #[test]
    fn a_destructive_git_command_offers_no_rule_and_defaults_to_no() {
        let mut mapper = Mapper::new(1);
        let params = json!({"threadId": "t", "turnId": "u", "itemId": "c", "command": "git reset --hard HEAD~1",
            "proposedExecpolicyAmendment": ["git", "reset"], "startedAtMs": 1});
        let (offer, updates) = mapper
            .request("p1", method::COMMAND_APPROVAL, &params, SystemTime::UNIX_EPOCH)
            .unwrap();
        assert_eq!(offer.picks, [Pick::Accept, Pick::Decline]);
        let entry = updates.iter().find_map(|u| match u {
            Update::Put {
                entry: EntryKind::PermissionRequest(p),
                ..
            } => Some(p.clone()),
            _ => None,
        });
        let entry = entry.unwrap();
        assert!(entry.default_to_no);
        assert_eq!(entry.options, [PermissionOption::AllowOnce, PermissionOption::Deny]);
        assert_eq!(entry.tool_call.as_deref(), Some("item:u:c"));
        let params = json!({"turnId": "u", "itemId": "c", "command": "cargo test",
            "proposedExecpolicyAmendment": ["cargo", "test"]});
        let (offer, _) = mapper
            .request("p2", method::COMMAND_APPROVAL, &params, SystemTime::UNIX_EPOCH)
            .unwrap();
        assert_eq!(offer.picks.len(), 4);
    }

    #[test]
    fn answers_go_out_by_question_id() {
        let ids = vec![("Which?".to_string(), "q1".to_string())];
        let given = [term_thread::Answer {
            question: "Which?".into(),
            answer: "Red".into(),
        }];
        assert_eq!(answers(&ids, &given), json!({"answers": {"q1": {"answers": ["Red"]}}}));
    }
}
