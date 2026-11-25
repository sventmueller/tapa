//! Claude Code's tools as the thread model shows them: kind, title and typed view from the input, output from the
//! structured result.

use std::time::Duration;

use serde_json::Value;
use term_thread::{Hunk, TextEdit, Todo, TodoStatus, ToolKind, ToolOutput, ToolView};

/// How a tool call appears in the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// As a tool card.
    Card,
    /// As the to-do list.
    Todos,
    /// As questions to the user.
    Question,
    /// As a plan to approve.
    Plan,
}

pub(crate) fn shape(name: &str) -> Shape {
    match name {
        "TodoWrite" => Shape::Todos,
        "AskUserQuestion" => Shape::Question,
        "ExitPlanMode" => Shape::Plan,
        _ => Shape::Card,
    }
}

/// Whether the tool starts a subagent.
pub(crate) fn starts_subagent(name: &str) -> bool {
    matches!(name, "Agent" | "Task")
}

fn text(input: &Value, field: &str) -> Option<String> {
    input.get(field).and_then(Value::as_str).map(str::to_string)
}

fn number(input: &Value, field: &str) -> Option<u64> {
    input.get(field).and_then(Value::as_u64)
}

fn flag(input: &Value, field: &str) -> bool {
    input.get(field).and_then(Value::as_bool).unwrap_or(false)
}

/// The kind, title and view of a call of `name` with `input`; the title is the call's subject, such as its path or
/// command, so the UI can put a localized verb in front of it.
pub(crate) fn describe(name: &str, input: &Value) -> (ToolKind, String, ToolView) {
    let path = || {
        text(input, "file_path")
            .or_else(|| text(input, "notebook_path"))
            .unwrap_or_default()
    };
    let (kind, view) = match name {
        "Bash" | "Monitor" => (
            ToolKind::Execute,
            ToolView::Command {
                command: text(input, "command").unwrap_or_default(),
                description: text(input, "description"),
                background: flag(input, "run_in_background"),
                timeout: number(input, "timeout").map(Duration::from_millis),
            },
        ),
        "BashOutput" | "KillShell" | "TaskStop" | "TaskOutput" => (ToolKind::Execute, ToolView::Other),
        "Read" | "NotebookRead" => (
            ToolKind::Read,
            ToolView::Read {
                path: path(),
                offset: number(input, "offset"),
                limit: number(input, "limit"),
            },
        ),
        "Edit" | "MultiEdit" | "NotebookEdit" => (
            ToolKind::Edit,
            ToolView::Edit {
                path: path(),
                edits: edits(input),
            },
        ),
        "Write" => (
            ToolKind::Edit,
            ToolView::Write {
                path: path(),
                content: text(input, "content").unwrap_or_default(),
            },
        ),
        "Grep" | "Glob" | "LS" => (
            ToolKind::Search,
            ToolView::Search {
                pattern: text(input, "pattern").unwrap_or_default(),
                path: text(input, "path"),
                glob: text(input, "glob"),
                results: None,
            },
        ),
        "ToolSearch" => (
            ToolKind::Search,
            ToolView::Search {
                pattern: text(input, "query").unwrap_or_default(),
                path: None,
                glob: None,
                results: None,
            },
        ),
        "WebFetch" => (
            ToolKind::Fetch,
            ToolView::Fetch {
                url: text(input, "url").unwrap_or_default(),
                prompt: text(input, "prompt"),
            },
        ),
        "WebSearch" | "web_search" => (
            ToolKind::Fetch,
            ToolView::WebSearch {
                query: text(input, "query").unwrap_or_default(),
            },
        ),
        "Agent" | "Task" => (
            ToolKind::Think,
            ToolView::Agent {
                description: text(input, "description").unwrap_or_default(),
                agent_type: text(input, "subagent_type"),
                prompt: text(input, "prompt").unwrap_or_default(),
                background: flag(input, "run_in_background"),
            },
        ),
        "TodoWrite" | "AskUserQuestion" => (ToolKind::Think, ToolView::Other),
        "ExitPlanMode" | "EnterPlanMode" => (ToolKind::SwitchMode, ToolView::Other),
        _ => match mcp(name) {
            Some((server, tool)) => (ToolKind::Mcp, ToolView::Mcp { server, tool }),
            None => (ToolKind::Other, ToolView::Other),
        },
    };
    let title = match &view {
        ToolView::Command {
            command, description, ..
        } => description.clone().unwrap_or_else(|| command.clone()),
        ToolView::Read { path, .. } | ToolView::Edit { path, .. } | ToolView::Write { path, .. } => path.clone(),
        ToolView::Search { pattern, .. } => pattern.clone(),
        ToolView::Fetch { url, .. } => url.clone(),
        ToolView::WebSearch { query } => query.clone(),
        ToolView::Agent { description, .. } => description.clone(),
        ToolView::Mcp { server, tool } => format!("{server}: {tool}"),
        ToolView::Other => name.to_string(),
    };
    let title = if title.is_empty() { name.to_string() } else { title };
    (kind, title, view)
}

/// The server and tool of an MCP tool name, `mcp__<server>__<tool>`.
fn mcp(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    Some((server.to_string(), tool.to_string()))
}

fn edits(input: &Value) -> Vec<TextEdit> {
    let one = |edit: &Value| TextEdit {
        old: text(edit, "old_string").unwrap_or_default(),
        new: text(edit, "new_string")
            .or_else(|| text(edit, "new_source"))
            .unwrap_or_default(),
        replace_all: flag(edit, "replace_all"),
    };
    match input.get("edits").and_then(Value::as_array) {
        Some(list) => list.iter().map(one).collect(),
        None => vec![one(input)],
    }
}

/// The to-do list in a TodoWrite input.
pub(crate) fn todos(input: &Value) -> Vec<Todo> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    Some(Todo {
                        content: text(item, "content")?,
                        active_form: text(item, "activeForm"),
                        status: match item.get("status").and_then(Value::as_str) {
                            Some("in_progress") => TodoStatus::InProgress,
                            Some("completed") => TodoStatus::Completed,
                            _ => TodoStatus::Pending,
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The text of a tool result's `content`: a string, or the text blocks of a list.
pub(crate) fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b.get("text").or_else(|| b.get("tool_name")).and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// The fields of a structured tool result that the mapping reads; a transcript read back keeps only these, so that
/// the whole files an edit or a read carries do not pass through memory.
pub(crate) const RESULT_FIELDS: [&str; 18] = [
    "answers",
    "status",
    "agentId",
    "totalTokens",
    "totalToolUseCount",
    "totalDurationMs",
    "stdout",
    "stderr",
    "interrupted",
    "filePath",
    "structuredPatch",
    "type",
    "content",
    "numMatches",
    "numLines",
    "numFiles",
    "matches",
    "backgroundTaskId",
];

/// What a call of `name` returned, from its structured result when the tool has one, else from the text.
pub(crate) fn output(name: &str, structured: Option<&Value>, content: &Value) -> ToolOutput {
    let structured = structured.filter(|v| v.is_object());
    match (name, structured) {
        ("Bash" | "BashOutput", Some(result)) if result.get("stdout").is_some() => ToolOutput::Terminal {
            stdout: text(result, "stdout").unwrap_or_default(),
            stderr: text(result, "stderr").unwrap_or_default(),
            interrupted: flag(result, "interrupted"),
        },
        ("Edit" | "MultiEdit" | "Write" | "NotebookEdit", Some(result)) => {
            let path = text(result, "filePath").unwrap_or_default();
            let mut hunks = patch(result);
            if hunks.is_empty()
                && result.get("type").and_then(Value::as_str) == Some("create")
                && let Some(content) = text(result, "content")
            {
                let lines: Vec<String> = content.lines().map(|l| format!("+{l}")).collect();
                hunks.push(Hunk {
                    old_start: 0,
                    old_lines: 0,
                    new_start: 1,
                    new_lines: lines.len() as u64,
                    lines,
                });
            }
            if hunks.is_empty() {
                ToolOutput::Text(result_text(content))
            } else {
                ToolOutput::Diff { path, hunks }
            }
        }
        _ => ToolOutput::Text(result_text(content)),
    }
}

fn patch(result: &Value) -> Vec<Hunk> {
    result
        .get("structuredPatch")
        .and_then(Value::as_array)
        .map(|hunks| {
            hunks
                .iter()
                .filter_map(|h| {
                    Some(Hunk {
                        old_start: number(h, "oldStart")?,
                        old_lines: number(h, "oldLines").unwrap_or(0),
                        new_start: number(h, "newStart")?,
                        new_lines: number(h, "newLines").unwrap_or(0),
                        lines: h
                            .get("lines")?
                            .as_array()?
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The number of results a search reported.
pub(crate) fn result_count(name: &str, structured: Option<&Value>) -> Option<u64> {
    let result = structured?;
    match name {
        "Grep" => ["numMatches", "numLines", "numFiles"]
            .iter()
            .find_map(|f| number(result, f)),
        "ToolSearch" => result.get("matches").and_then(Value::as_array).map(|m| m.len() as u64),
        "Glob" | "LS" => number(result, "numFiles"),
        _ => None,
    }
}

/// The background task a call became, from its structured result.
pub(crate) fn background_task(structured: Option<&Value>) -> Option<String> {
    structured.and_then(|r| text(r, "backgroundTaskId"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_bash_call_shows_its_description_and_background_flag() {
        let (kind, title, view) = describe(
            "Bash",
            &json!({"command": "sleep 30", "description": "Wait", "run_in_background": true, "timeout": 5000}),
        );
        assert_eq!(kind, ToolKind::Execute);
        assert_eq!(title, "Wait");
        assert_eq!(
            view,
            ToolView::Command {
                command: "sleep 30".into(),
                description: Some("Wait".into()),
                background: true,
                timeout: Some(Duration::from_secs(5)),
            }
        );
    }

    #[test]
    fn edits_of_one_or_many_replacements_are_listed() {
        let (_, title, view) = describe(
            "Edit",
            &json!({"file_path": "/a.rs", "old_string": "x", "new_string": "y"}),
        );
        assert_eq!(title, "/a.rs");
        let ToolView::Edit { edits, .. } = view else { panic!() };
        assert_eq!(edits[0].new, "y");
        let (_, _, view) = describe(
            "MultiEdit",
            &json!({"file_path": "/a.rs", "edits": [{"old_string": "a", "new_string": "b"}, {"old_string": "c", "new_string": "d", "replace_all": true}]}),
        );
        let ToolView::Edit { edits, .. } = view else { panic!() };
        assert_eq!(edits.len(), 2);
        assert!(edits[1].replace_all);
    }

    #[test]
    fn mcp_tools_name_their_server_and_unknown_tools_their_name() {
        let (kind, title, _) = describe("mcp__context7__query-docs", &json!({}));
        assert_eq!((kind, title.as_str()), (ToolKind::Mcp, "context7: query-docs"));
        let (kind, title, view) = describe("FutureTool", &json!({"x": 1}));
        assert_eq!(
            (kind, title.as_str(), view),
            (ToolKind::Other, "FutureTool", ToolView::Other)
        );
    }

    #[test]
    fn a_created_file_shows_as_a_diff_of_added_lines() {
        let output = output(
            "Write",
            Some(&json!({"type": "create", "filePath": "/w/a.txt", "content": "one\ntwo\n", "structuredPatch": []})),
            &json!("File created"),
        );
        let ToolOutput::Diff { hunks, .. } = output else {
            panic!()
        };
        assert_eq!(hunks[0].lines, ["+one", "+two"]);
        assert_eq!(hunks[0].new_lines, 2);
    }

    #[test]
    fn a_result_without_structure_falls_back_to_its_text() {
        let output = output(
            "Read",
            None,
            &json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]),
        );
        assert_eq!(output, ToolOutput::Text("a\nb".into()));
        assert_eq!(result_count("Grep", Some(&json!({"numFiles": 3}))), Some(3));
        assert_eq!(
            result_count("Glob", Some(&json!({"numFiles": 2, "filenames": []}))),
            Some(2)
        );
    }

    #[test]
    fn todos_keep_their_order_and_status() {
        let list = todos(&json!({"todos": [
            {"content": "pick", "activeForm": "Picking", "status": "in_progress"},
            {"content": "say", "status": "pending"},
            {"status": "completed"}
        ]}));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, TodoStatus::InProgress);
        assert_eq!(list[0].active_form.as_deref(), Some("Picking"));
    }
}
