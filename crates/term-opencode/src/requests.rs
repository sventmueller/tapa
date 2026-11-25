//! What the server asks the user: permissions (`permission.asked`) and forms (`form.created`, which OpenCode's
//! `question` tool and MCP elicitations use). Each becomes an entry of the thread and an `Offer` that knows how the
//! answer goes on the wire. Requests are named by OpenCode's own ids (`per_…`, `frm_…`), so an answer stays valid
//! for a session resumed while one waits.

use std::time::SystemTime;

use serde_json::{Map, Value, json};
use term_thread::{
    Answer, Decision, EntryKind, PermissionOption, PermissionRequest, Question, QuestionItem, QuestionOption,
    RequestState, RuleScope, Status, ToolPatch, ToolStatus, Update,
};

use crate::mapping::{Mapper, put, text, tool_key};

/// One answer to a permission, in the order the entry offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    /// `once`.
    Once,
    /// `always`: OpenCode saves the request's `save` patterns as allowed.
    Always,
    /// `reject`.
    Reject,
}

impl Pick {
    /// The `Permission.Reply` this stands for.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Pick::Once => "once",
            Pick::Always => "always",
            Pick::Reject => "reject",
        }
    }

    fn option(self, n: usize, rule: &str) -> PermissionOption {
        match self {
            Pick::Once => PermissionOption::AllowOnce,
            Pick::Always => PermissionOption::AllowAlways {
                suggestion: n,
                rule: rule.to_string(),
                scope: RuleScope::Project,
            },
            Pick::Reject => PermissionOption::Deny,
        }
    }

    /// What the thread records for this answer.
    pub(crate) fn decision(self) -> Decision {
        match self {
            Pick::Once => Decision::Allowed {
                always: false,
                edited: false,
            },
            Pick::Always => Decision::Allowed {
                always: true,
                edited: false,
            },
            Pick::Reject => Decision::Denied { message: None },
        }
    }
}

/// How the answer to one field of a form is written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FieldKind {
    /// Text, picked from options (label to value) when there are any.
    Text(Vec<(String, String)>),
    /// A number; whole when `true`.
    Number(bool),
    /// Yes or no.
    Boolean,
    /// Several of the options, label to value.
    Multiselect(Vec<(String, String)>),
}

/// A field of a form, by the question the entry shows for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub(crate) key: String,
    pub(crate) question: String,
    pub(crate) kind: FieldKind,
}

/// What a request is about, which decides the shape of its answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum AskKind {
    /// A permission.
    Permission,
    /// A form, with its fields.
    Form(Vec<Field>),
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

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn options(field: &Value) -> Vec<(String, String, Option<String>)> {
    field
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|o| {
            let value = text(o, "value")?.to_string();
            let label = text(o, "label").map_or_else(|| value.clone(), str::to_string);
            Some((label, value, text(o, "description").map(str::to_string)))
        })
        .collect()
}

/// The questions of a form's fields; hidden fields and links to an outside page are left out.
fn form_fields(fields: &[Value], title: &str) -> (Vec<QuestionItem>, Vec<Field>) {
    let mut items = Vec::new();
    let mut read = Vec::new();
    for field in fields {
        let Some(key) = text(field, "key") else {
            continue;
        };
        if field.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let choices = options(field);
        let pairs = || choices.iter().map(|(l, v, _)| (l.clone(), v.clone())).collect();
        let kind = match text(field, "type") {
            Some("string") => FieldKind::Text(pairs()),
            Some("number") => FieldKind::Number(false),
            Some("integer") => FieldKind::Number(true),
            Some("boolean") => FieldKind::Boolean,
            Some("multiselect") => FieldKind::Multiselect(pairs()),
            _ => continue,
        };
        let question = text(field, "title")
            .or_else(|| text(field, "description"))
            .unwrap_or(key)
            .to_string();
        items.push(QuestionItem {
            header: items.is_empty().then(|| title.to_string()).filter(|t| !t.is_empty()),
            question: question.clone(),
            options: choices
                .iter()
                .map(|(label, _, description)| QuestionOption {
                    label: label.clone(),
                    description: description.clone(),
                })
                .collect(),
            multi_select: matches!(kind, FieldKind::Multiselect(_)),
        });
        read.push(Field {
            key: key.to_string(),
            question,
            kind,
        });
    }
    (items, read)
}

impl Mapper {
    /// The entry and offer for a `permission.asked` event's `data`, with OpenCode's id of the request.
    pub(crate) fn permission(&mut self, data: &Value, at: SystemTime) -> Option<(String, Offer, Vec<Update>)> {
        let id = text(data, "id")?.to_string();
        let action = text(data, "action").unwrap_or_default();
        let resources = strings(data.get("resources"));
        let save = strings(data.get("save"));
        let item = data
            .get("source")
            .and_then(|s| Some(tool_key(text(s, "messageID")?, text(s, "id")?)));
        let destructive = matches!(action, "shell" | "bash")
            && resources
                .iter()
                .any(|r| term_thread::shell::destructive_git(r).is_some());
        let picks = if destructive {
            vec![Pick::Once, Pick::Reject]
        } else {
            vec![Pick::Once, Pick::Always, Pick::Reject]
        };
        let rule = if save.is_empty() {
            resources.join(" ")
        } else {
            save.join(" ")
        };
        let entry = PermissionRequest {
            request_id: id.clone(),
            tool_call: item.clone(),
            tool_name: crate::mapping::claude_name(action),
            title: Some(resources.join(", ")).filter(|t| !t.is_empty()),
            description: None,
            reason: text(data, "message").map(str::to_string),
            blocked_path: None,
            input: data.get("metadata").cloned().unwrap_or_else(|| json!({})),
            options: picks.iter().enumerate().map(|(n, p)| p.option(n, &rule)).collect(),
            default_to_no: destructive,
            state: RequestState::Pending,
        };
        let key = format!("permission-{id}");
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
        let offer = Offer {
            kind: AskKind::Permission,
            picks,
            key,
            item,
        };
        Some((id, offer, out))
    }

    /// The entry and offer for the `form` of a `form.created` event, with OpenCode's id of the form.
    pub(crate) fn form(&mut self, form: &Value, at: SystemTime) -> Option<(String, Offer, Vec<Update>)> {
        let id = text(form, "id")?.to_string();
        let fields = form.get("fields").and_then(Value::as_array)?;
        let (items, read) = form_fields(fields, text(form, "title").unwrap_or_default());
        let item = form
            .get("metadata")
            .and_then(|m| m.get("tool"))
            .and_then(|t| text(t, "id"))
            .and_then(|call| self.tool_entry(call));
        let key = format!("question-{id}");
        let entry = Question {
            request_id: Some(id.clone()),
            tool_call: item.clone().unwrap_or_default(),
            items,
            state: RequestState::Pending,
        };
        let out = vec![
            put(key.clone(), at, EntryKind::Question(entry)),
            Update::Ask {
                key: key.clone(),
                request_id: id.clone(),
            },
            Update::Status(Status::RequiresAction),
        ];
        let offer = Offer {
            kind: AskKind::Form(read),
            picks: Vec::new(),
            key,
            item,
        };
        Some((id, offer, out))
    }
}

/// The updates once the user answered the request behind `offer` with `decision`; `still_asking` says whether
/// other requests wait.
pub(crate) fn decided(offer: &Offer, decision: Decision, still_asking: bool) -> Vec<Update> {
    let allowed = matches!(decision, Decision::Allowed { .. } | Decision::Answered(_));
    let mut out = Vec::new();
    if let Some(item) = offer.item.as_ref().filter(|_| offer.kind == AskKind::Permission) {
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

/// The value an answer `given` to a field of `kind` takes; `None` when it does not fit.
fn value(kind: &FieldKind, given: &str) -> Option<Value> {
    let pick = |options: &[(String, String)], part: &str| {
        options
            .iter()
            .find(|(label, value)| label == part || value == part)
            .map_or_else(|| part.to_string(), |(_, value)| value.clone())
    };
    let given = given.trim();
    match kind {
        FieldKind::Text(options) => Some(json!(pick(options, given))),
        FieldKind::Number(true) => given.parse::<i64>().ok().map(Value::from),
        FieldKind::Number(false) => given.parse::<f64>().ok().map(Value::from),
        FieldKind::Boolean => match given.to_ascii_lowercase().as_str() {
            "true" | "yes" | "y" | "1" | "ja" => Some(json!(true)),
            "false" | "no" | "n" | "0" | "nein" => Some(json!(false)),
            _ => None,
        },
        FieldKind::Multiselect(options) => Some(json!(
            given
                .split(", ")
                .filter(|p| !p.is_empty())
                .map(|p| pick(options, p))
                .collect::<Vec<_>>()
        )),
    }
}

/// The `Form.Reply` for `answers`, matched to the fields by their question.
pub(crate) fn form_reply(fields: &[Field], answers: &[Answer]) -> Value {
    let mut answer = Map::new();
    for given in answers {
        if let Some(field) = fields.iter().find(|f| f.question == given.question)
            && let Some(value) = value(&field.kind, &given.answer)
        {
            answer.insert(field.key.clone(), value);
        }
    }
    json!({"answer": answer})
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    fn entry(updates: &[Update]) -> PermissionRequest {
        updates
            .iter()
            .find_map(|u| match u {
                Update::Put {
                    entry: EntryKind::PermissionRequest(p),
                    ..
                } => Some(p.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn a_permission_offers_once_always_and_reject_and_names_its_tool_card() {
        let mut mapper = Mapper::new(1);
        let data = json!({"id": "per_1", "sessionID": "ses_1", "action": "shell", "resources": ["cargo test"],
            "save": ["cargo *"], "source": {"type": "tool", "messageID": "msg_1", "id": "call_1"},
            "message": "Run the tests"});
        let (id, offer, updates) = mapper.permission(&data, UNIX_EPOCH).unwrap();
        assert_eq!(id, "per_1");
        assert_eq!(offer.picks, [Pick::Once, Pick::Always, Pick::Reject]);
        assert_eq!(offer.item.as_deref(), Some("tool:msg_1:call_1"));
        let asked = entry(&updates);
        assert_eq!(asked.tool_name, "Bash");
        assert_eq!(asked.title.as_deref(), Some("cargo test"));
        assert_eq!(asked.reason.as_deref(), Some("Run the tests"));
        assert!(matches!(&asked.options[1], PermissionOption::AllowAlways { rule, .. } if rule == "cargo *"));
        let git = json!({"id": "per_2", "sessionID": "ses_1", "action": "shell",
            "resources": ["git reset --hard HEAD~1"], "save": ["git reset *"]});
        let (_, offer, updates) = mapper.permission(&git, UNIX_EPOCH).unwrap();
        assert_eq!(offer.picks, [Pick::Once, Pick::Reject]);
        assert!(entry(&updates).default_to_no);
        assert_eq!(Pick::Always.wire(), "always");
    }

    #[test]
    fn a_form_asks_its_fields_and_answers_by_key_with_option_values() {
        let mut mapper = Mapper::new(1);
        let form = json!({"id": "frm_1", "sessionID": "ses_1", "title": "Colour", "fields": [
            {"key": "color", "type": "string", "title": "Which colour?",
             "options": [{"value": "red", "label": "Red", "description": "warm"}, {"value": "blue", "label": "Blue"}]},
            {"key": "tags", "type": "multiselect", "title": "Tags",
             "options": [{"value": "a", "label": "Alpha"}, {"value": "b", "label": "Beta"}], "custom": true},
            {"key": "count", "type": "integer", "description": "How many?"},
            {"key": "sure", "type": "boolean"},
            {"key": "secret", "type": "string", "hidden": true},
            {"key": "page", "type": "external", "url": "https://example.test"}]});
        let (id, offer, updates) = mapper.form(&form, UNIX_EPOCH).unwrap();
        assert_eq!(id, "frm_1");
        let Some(Update::Put {
            entry: EntryKind::Question(question),
            ..
        }) = updates.first()
        else {
            panic!("{updates:?}")
        };
        let asked: Vec<&str> = question.items.iter().map(|i| i.question.as_str()).collect();
        assert_eq!(asked, ["Which colour?", "Tags", "How many?", "sure"]);
        assert_eq!(question.items[0].header.as_deref(), Some("Colour"));
        assert!(question.items[1].multi_select);
        let AskKind::Form(fields) = &offer.kind else { panic!() };
        let answer = |question: &str, answer: &str| Answer {
            question: question.into(),
            answer: answer.into(),
        };
        let reply = form_reply(
            fields,
            &[
                answer("Which colour?", "Red"),
                answer("Tags", "Beta, Gamma"),
                answer("How many?", "3"),
                answer("sure", "yes"),
                answer("Unknown", "x"),
            ],
        );
        assert_eq!(
            reply,
            json!({"answer": {"color": "red", "tags": ["b", "Gamma"], "count": 3, "sure": true}})
        );
        assert_eq!(
            form_reply(fields, &[answer("How many?", "many")]),
            json!({"answer": {}})
        );
    }
}
