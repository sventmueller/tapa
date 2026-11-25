//! OpenCode's tools as cards. The names and inputs are those of OpenCode 2.2.6's built-in tools, which the schema does
//! not describe (a tool's input is any object there): `shell`, `read`, `write`, `edit`, `patch`, `glob`, `grep`,
//! `webfetch`, `websearch`, `subagent`, `todowrite`, `question` and `skill`, with the 1.x names `bash`, `task` and
//! `apply_patch` read the same. Any other tool, such as an MCP server's, keeps its own name.

use std::time::Duration;

use serde_json::Value;
use term_thread::{TextEdit, Todo, TodoStatus, ToolKind, ToolOutput, ToolView};

use crate::diff;

/// What a card shows of a tool's input.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Card {
    pub(crate) name: String,
    pub(crate) kind: ToolKind,
    pub(crate) title: String,
    pub(crate) view: ToolView,
}

fn field<'a>(input: &'a Value, names: &[&str]) -> Option<&'a str> {
    names.iter().find_map(|n| input.get(*n).and_then(Value::as_str))
}

fn string(input: &Value, names: &[&str]) -> String {
    field(input, names).unwrap_or_default().to_string()
}

/// The files a patch in OpenCode's format adds, updates or deletes.
fn patched_files(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|line| {
            ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(str::trim)
        .collect()
}

fn card(name: &str, kind: ToolKind, title: String, view: ToolView) -> Card {
    Card {
        name: name.to_string(),
        kind,
        title,
        view,
    }
}

fn command(input: &Value) -> Card {
    let line = string(input, &["command"]);
    let view = ToolView::Command {
        command: line.clone(),
        description: field(input, &["description"]).map(str::to_string),
        background: input.get("background").and_then(Value::as_bool).unwrap_or(false),
        timeout: input
            .get("timeout")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
            .map(Duration::from_millis),
    };
    // Named as Claude Code names its tools, so the cards read the same for every agent.
    card("Bash", ToolKind::Execute, line, view)
}

fn file_tool(tool: &str, input: &Value) -> Card {
    let path = string(input, &["path", "filePath"]);
    match tool {
        "read" => {
            let view = ToolView::Read {
                path: path.clone(),
                offset: input.get("offset").and_then(Value::as_u64),
                limit: input.get("limit").and_then(Value::as_u64),
            };
            card("Read", ToolKind::Read, path, view)
        }
        "write" => {
            let view = ToolView::Write {
                path: path.clone(),
                content: string(input, &["content"]),
            };
            card("Write", ToolKind::Edit, path, view)
        }
        _ => {
            let edits = vec![TextEdit {
                old: string(input, &["oldString"]),
                new: string(input, &["newString"]),
                replace_all: input.get("replaceAll").and_then(Value::as_bool).unwrap_or(false),
            }];
            let view = ToolView::Edit {
                path: path.clone(),
                edits,
            };
            card("Edit", ToolKind::Edit, path, view)
        }
    }
}

fn search(tool: &str, input: &Value) -> Card {
    let pattern = string(input, &["pattern"]);
    let path = field(input, &["path"]).map(str::to_string);
    if tool == "glob" {
        let view = ToolView::Search {
            pattern: String::new(),
            path,
            glob: Some(pattern.clone()),
            results: None,
        };
        return card("Glob", ToolKind::Search, pattern, view);
    }
    let view = ToolView::Search {
        pattern: pattern.clone(),
        path,
        glob: field(input, &["include"]).map(str::to_string),
        results: None,
    };
    card("Grep", ToolKind::Search, pattern, view)
}

fn subagent(input: &Value) -> Card {
    let prompt = string(input, &["prompt"]);
    let description = field(input, &["description"])
        .map(str::to_string)
        .unwrap_or_else(|| term_thread::title_from(&prompt));
    let view = ToolView::Agent {
        description: description.clone(),
        agent_type: field(input, &["agent", "subagent_type"]).map(str::to_string),
        prompt,
        background: false,
    };
    card("Task", ToolKind::Think, description, view)
}

/// The card of the tool `tool` with `input`, which may still be empty while the model writes it.
pub(crate) fn card_for(tool: &str, input: &Value) -> Card {
    match tool {
        "shell" | "bash" => command(input),
        "read" | "write" | "edit" => file_tool(tool, input),
        "patch" | "apply_patch" => {
            let files = patched_files(field(input, &["patchText"]).unwrap_or_default());
            let title = if files.is_empty() {
                "patch".to_string()
            } else {
                files.join(", ")
            };
            card("Edit", ToolKind::Edit, title, ToolView::Other)
        }
        "glob" | "grep" => search(tool, input),
        "webfetch" => {
            let url = string(input, &["url"]);
            let view = ToolView::Fetch {
                url: url.clone(),
                prompt: None,
            };
            card("WebFetch", ToolKind::Fetch, url, view)
        }
        "websearch" => {
            let query = string(input, &["query"]);
            let view = ToolView::WebSearch { query: query.clone() };
            card("WebSearch", ToolKind::Fetch, query, view)
        }
        "subagent" | "task" => subagent(input),
        "todowrite" => card("TodoWrite", ToolKind::Other, "todowrite".into(), ToolView::Other),
        "question" => card("AskUserQuestion", ToolKind::Other, "question".into(), ToolView::Other),
        "skill" => {
            let id = string(input, &["id", "name"]);
            card("Skill", ToolKind::Other, id, ToolView::Other)
        }
        other => card(other, ToolKind::Other, other.to_string(), ToolView::Other),
    }
}

/// The text parts of a tool's `content`, one per line.
pub(crate) fn content_text(content: Option<&Value>) -> Option<String> {
    let parts: Vec<&str> = content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// The one changed file of an edit's metadata as a diff, from its `files` of `FileDiff.Info`.
fn file_diff(metadata: Option<&Value>) -> Option<ToolOutput> {
    let files = metadata?.get("files")?.as_array()?;
    let [one] = files.as_slice() else {
        return None;
    };
    let path = one.get("file")?.as_str()?.to_string();
    let hunks = diff::unified(one.get("patch")?.as_str()?);
    (!hunks.is_empty()).then_some(ToolOutput::Diff { path, hunks })
}

/// What a settled tool returned: a command's output as a terminal's, a file change as its diff, else its text.
pub(crate) fn output(
    tool: &str,
    input: &Value,
    content: Option<&Value>,
    metadata: Option<&Value>,
) -> Option<ToolOutput> {
    let text = content_text(content);
    match tool {
        "shell" | "bash" => Some(ToolOutput::Terminal {
            stdout: text.unwrap_or_default(),
            stderr: String::new(),
            interrupted: false,
        }),
        "edit" | "patch" | "apply_patch" => file_diff(metadata).or(text.map(ToolOutput::Text)),
        "write" => {
            let path = string(input, &["path", "filePath"]);
            let hunks = diff::whole(field(input, &["content"]).unwrap_or_default(), false);
            if hunks.is_empty() {
                text.map(ToolOutput::Text)
            } else {
                Some(ToolOutput::Diff { path, hunks })
            }
        }
        _ => text.map(ToolOutput::Text),
    }
}

/// The to-do list a `todowrite` call sets; a cancelled item counts as done.
pub(crate) fn todos(input: &Value) -> Option<Vec<Todo>> {
    let items = input.get("todos")?.as_array()?;
    Some(
        items
            .iter()
            .filter_map(|item| {
                Some(Todo {
                    content: item.get("content")?.as_str()?.to_string(),
                    active_form: None,
                    status: match item.get("status").and_then(Value::as_str) {
                        Some("in_progress") => TodoStatus::InProgress,
                        Some("completed" | "cancelled") => TodoStatus::Completed,
                        _ => TodoStatus::Pending,
                    },
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn built_in_tools_get_the_names_and_views_claude_code_uses() {
        let shell = card_for(
            "shell",
            &json!({"command": "ls -la", "description": "List", "timeout": 5000}),
        );
        assert_eq!((shell.name.as_str(), shell.kind), ("Bash", ToolKind::Execute));
        assert!(matches!(&shell.view, ToolView::Command { timeout: Some(t), .. } if *t == Duration::from_secs(5)));
        let edit = card_for("edit", &json!({"path": "a.txt", "oldString": "two", "newString": "2"}));
        assert!(matches!(&edit.view, ToolView::Edit { edits, .. } if edits[0].new == "2"));
        let read = card_for("read", &json!({"path": "src/a.rs", "offset": 10, "limit": 5}));
        assert_eq!(read.view.line_range(), Some((10, Some(14))));
        let grep = card_for("grep", &json!({"pattern": "fn main", "include": "*.rs"}));
        assert!(matches!(&grep.view, ToolView::Search { glob: Some(g), .. } if g == "*.rs"));
        let glob = card_for("glob", &json!({"pattern": "**/*.rs"}));
        assert_eq!((glob.name.as_str(), glob.title.as_str()), ("Glob", "**/*.rs"));
        let patch = card_for(
            "patch",
            &json!({"patchText": "*** Begin Patch\n*** Update File: a.rs\n*** Add File: b.rs\n"}),
        );
        assert_eq!(patch.title, "a.rs, b.rs");
        let task = card_for(
            "subagent",
            &json!({"agent": "general", "description": "Find it", "prompt": "Look"}),
        );
        assert!(matches!(&task.view, ToolView::Agent { agent_type: Some(a), .. } if a == "general"));
        assert_eq!(card_for("bash", &json!({"command": "pwd"})).name, "Bash");
        assert_eq!(card_for("docs_search", &json!({})).name, "docs_search");
        assert_eq!(
            card_for("websearch", &json!({"query": "q"})).view,
            ToolView::WebSearch { query: "q".into() }
        );
    }

    #[test]
    fn outputs_follow_the_tool() {
        let content = json!([{"type": "text", "text": "a.txt"}, {"type": "file", "uri": "x", "mime": "y"}]);
        assert!(matches!(output("shell", &json!({}), Some(&content), None),
            Some(ToolOutput::Terminal { stdout, .. }) if stdout == "a.txt"));
        let metadata = json!({"files": [{"file": "a.txt", "patch": "@@ -1,2 +1,2 @@\n one\n-two\n+2\n",
            "additions": 1, "deletions": 1, "status": "modified"}]});
        let Some(ToolOutput::Diff { path, hunks }) = output("edit", &json!({}), Some(&content), Some(&metadata)) else {
            panic!()
        };
        assert_eq!((path.as_str(), hunks[0].lines.len()), ("a.txt", 3));
        assert_eq!(
            output("edit", &json!({}), Some(&content), None),
            Some(ToolOutput::Text("a.txt".into()))
        );
        let written = output("write", &json!({"path": "n.txt", "content": "x\ny\n"}), None, None);
        assert!(matches!(written, Some(ToolOutput::Diff { hunks, .. }) if hunks[0].lines == ["+x", "+y"]));
        assert_eq!(output("read", &json!({}), None, None), None);
    }

    #[test]
    fn todos_read_with_their_states() {
        let list = todos(&json!({"todos": [{"content": "a", "status": "in_progress"},
            {"content": "b", "status": "cancelled"}, {"content": "c", "status": "pending"}, {"status": "x"}]}))
        .unwrap();
        let states: Vec<TodoStatus> = list.iter().map(|t| t.status).collect();
        assert_eq!(
            states,
            [TodoStatus::InProgress, TodoStatus::Completed, TodoStatus::Pending]
        );
        assert_eq!(todos(&json!({})), None);
    }
}
