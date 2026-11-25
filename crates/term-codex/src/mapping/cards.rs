//! Tool items as cards: commands, file changes, MCP and dynamic tool calls, web searches, viewed images and
//! subagent calls.

use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use term_thread::{ToolCall, ToolKind, ToolOutput, ToolPatch, ToolStatus, ToolView};

use crate::diff;
use crate::protocol::{CommandAction, FileChange, Item, text};

/// What a card shows of a tool item at one point of its life.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Card {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: ToolKind,
    pub(crate) title: String,
    pub(crate) input: Value,
    pub(crate) view: ToolView,
    pub(crate) status: ToolStatus,
    pub(crate) output: Option<ToolOutput>,
    pub(crate) elapsed: Option<Duration>,
}

/// The card status for the item status `status`; a declined command or change counts as failed.
fn status(status: &str) -> ToolStatus {
    match status {
        "completed" => ToolStatus::Completed,
        "failed" | "declined" | "interrupted" => ToolStatus::Failed,
        _ => ToolStatus::InProgress,
    }
}

impl Card {
    fn new(name: &str, kind: ToolKind, title: String, input: Value, view: ToolView, state: &str) -> Card {
        Card {
            id: String::new(),
            name: name.to_string(),
            kind,
            title,
            input,
            view,
            status: status(state),
            output: None,
            elapsed: None,
        }
    }

    pub(crate) fn is_over(&self) -> bool {
        matches!(self.status, ToolStatus::Completed | ToolStatus::Failed)
    }

    /// The card as a new entry that started at `at` and, when `ended`, also ended then.
    pub(crate) fn call(self, at: SystemTime, ended: bool) -> ToolCall {
        let status = if ended && !self.is_over() {
            ToolStatus::Completed
        } else {
            self.status
        };
        let mut call = ToolCall::new(self.id, self.name, self.kind);
        call.title = self.title;
        call.status = status;
        call.input = self.input;
        call.view = self.view;
        call.output = self.output;
        call.started = Some(at);
        call.ended = ended.then_some(at);
        call.elapsed = self.elapsed;
        call
    }

    /// The change to the card of the same item when it completed at `at`.
    pub(crate) fn patch(self, at: SystemTime) -> ToolPatch {
        let status = if self.is_over() {
            self.status
        } else {
            ToolStatus::Completed
        };
        ToolPatch {
            input: Some(self.input),
            title: Some(self.title),
            view: Some(self.view).filter(|v| *v != ToolView::Other),
            status: Some(status),
            output: self.output,
            ended: Some(at),
            elapsed: self.elapsed,
            ..ToolPatch::default()
        }
    }
}

fn command(
    command: &str,
    cwd: &str,
    actions: &[CommandAction],
    state: &str,
    output: Option<&String>,
    elapsed: Option<u64>,
) -> Card {
    let input = json!({"command": command, "cwd": cwd});
    let (kind, view) = match actions {
        [CommandAction::Read { path }] => (
            ToolKind::Read,
            ToolView::Read {
                path: path.clone(),
                offset: None,
                limit: None,
            },
        ),
        [CommandAction::Search { query, path }] => (
            ToolKind::Search,
            ToolView::Search {
                pattern: query.clone().unwrap_or_default(),
                path: path.clone(),
                glob: None,
                results: None,
            },
        ),
        [CommandAction::ListFiles { path }] => (
            ToolKind::Search,
            ToolView::Search {
                pattern: String::new(),
                path: path.clone(),
                glob: None,
                results: None,
            },
        ),
        _ => (
            ToolKind::Execute,
            ToolView::Command {
                command: command.to_string(),
                description: None,
                background: false,
                timeout: None,
            },
        ),
    };
    // Named as Claude Code names its tools, so the cards read the same for every agent.
    let mut card = Card::new("Bash", kind, command.to_string(), input, view, state);
    card.output = output.map(|stdout| ToolOutput::Terminal {
        stdout: stdout.clone(),
        stderr: String::new(),
        interrupted: false,
    });
    card.elapsed = elapsed.map(Duration::from_millis);
    card
}

/// The card of a file change item with `changes`.
pub(crate) fn file_change(changes: &[FileChange], state: &str) -> Card {
    let input = json!({"changes": changes.iter().map(|c| json!({"path": c.path, "kind": c.kind, "diff": c.diff}))
        .collect::<Vec<_>>()});
    let kind = if !changes.is_empty() && changes.iter().all(|c| c.kind() == "delete") {
        ToolKind::Delete
    } else {
        ToolKind::Edit
    };
    let title = match changes {
        [one] => one.path.clone(),
        many => many.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", "),
    };
    let view = match changes {
        [one] if one.kind() == "add" => ToolView::Write {
            path: one.path.clone(),
            content: one.diff.clone(),
        },
        [one] => ToolView::Edit {
            path: one.path.clone(),
            edits: Vec::new(),
        },
        _ => ToolView::Other,
    };
    let name = if changes.iter().all(|c| c.kind() == "add") {
        "Write"
    } else {
        "Edit"
    };
    let mut card = Card::new(name, kind, title, input, view, state);
    card.output = match changes {
        [] => None,
        [one] => Some(ToolOutput::Diff {
            path: one.path.clone(),
            hunks: match one.kind() {
                "add" => diff::whole(&one.diff, false),
                "delete" => diff::whole(&one.diff, true),
                _ => diff::unified(&one.diff),
            },
        }),
        many => Some(ToolOutput::Text(
            many.iter()
                .map(|c| format!("{}\n{}", c.path, c.diff))
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    };
    card
}

/// The text of MCP or dynamic tool content: its text parts, one per line.
fn content_text(content: &[Value]) -> Option<String> {
    let parts: Vec<&str> = content.iter().filter_map(|c| text(c, "text")).collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn mcp(item: &Item) -> Option<Card> {
    let Item::McpToolCall {
        server,
        tool,
        arguments,
        status,
        result,
        error,
        duration_ms,
        ..
    } = item
    else {
        return None;
    };
    let view = ToolView::Mcp {
        server: server.clone(),
        tool: tool.clone(),
    };
    let mut card = Card::new(tool, ToolKind::Mcp, tool.clone(), arguments.clone(), view, status);
    let result_text = result
        .as_ref()
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|c| content_text(c));
    let error_text = error.as_ref().and_then(|e| text(e, "message")).map(str::to_string);
    card.output = error_text.or(result_text).map(ToolOutput::Text);
    card.elapsed = duration_ms.map(Duration::from_millis);
    Some(card)
}

fn web_search(query: &str, action: Option<&Value>) -> Card {
    let url = action
        .filter(|a| text(a, "type") == Some("openPage"))
        .and_then(|a| text(a, "url"));
    let (title, view) = match url {
        Some(url) => (
            url.to_string(),
            ToolView::Fetch {
                url: url.to_string(),
                prompt: None,
            },
        ),
        None => (
            query.to_string(),
            ToolView::WebSearch {
                query: query.to_string(),
            },
        ),
    };
    let input = json!({"query": query, "action": action});
    Card::new("WebSearch", ToolKind::Fetch, title, input, view, "inProgress")
}

/// The card of a tool item; `None` for items that are no tool.
pub(crate) fn card(item: &Item) -> Option<Card> {
    let mut card = tool_card(item)?;
    card.id = item.id().unwrap_or_default().to_string();
    Some(card)
}

fn tool_card(item: &Item) -> Option<Card> {
    Some(match item {
        Item::CommandExecution {
            command: line,
            cwd,
            command_actions,
            status,
            aggregated_output,
            duration_ms,
            ..
        } => command(
            line,
            cwd,
            command_actions,
            status,
            aggregated_output.as_ref(),
            *duration_ms,
        ),
        Item::FileChange { changes, status, .. } => file_change(changes, status),
        Item::McpToolCall { .. } => mcp(item)?,
        Item::DynamicToolCall {
            tool,
            arguments,
            status,
            content_items,
            duration_ms,
            ..
        } => {
            let mut card = Card::new(
                tool,
                ToolKind::Other,
                tool.clone(),
                arguments.clone(),
                ToolView::Other,
                status,
            );
            card.output = content_items.as_deref().and_then(content_text).map(ToolOutput::Text);
            card.elapsed = duration_ms.map(Duration::from_millis);
            card
        }
        Item::CollabAgentToolCall {
            tool, prompt, status, ..
        } => {
            let prompt = prompt.clone().unwrap_or_default();
            Card::new(
                tool,
                ToolKind::Think,
                tool.clone(),
                json!({"prompt": prompt}),
                ToolView::Other,
                status,
            )
        }
        Item::WebSearch { query, action, .. } => web_search(query, action.as_ref()),
        Item::ImageView { path, .. } => {
            let view = ToolView::Read {
                path: path.clone(),
                offset: None,
                limit: None,
            };
            Card::new(
                "imageView",
                ToolKind::Read,
                path.clone(),
                json!({"path": path}),
                view,
                "completed",
            )
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(value: Value) -> Item {
        Item::read(&value).unwrap()
    }

    #[test]
    fn a_command_that_only_reads_or_searches_gets_that_card() {
        let item = read(
            json!({"type": "commandExecution", "id": "c", "command": "cat a.rs", "cwd": "/w",
            "commandActions": [{"type": "read", "command": "cat a.rs", "name": "a.rs", "path": "/w/a.rs"}],
            "status": "completed", "aggregatedOutput": "fn main() {}", "exitCode": 0, "durationMs": 12}),
        );
        let shown = card(&item).unwrap();
        assert_eq!(shown.kind, ToolKind::Read);
        assert!(matches!(&shown.view, ToolView::Read { path, .. } if path == "/w/a.rs"));
        assert_eq!(shown.elapsed, Some(Duration::from_millis(12)));
        let item = read(
            json!({"type": "commandExecution", "id": "c", "command": "cargo test", "cwd": "/w",
            "commandActions": [{"type": "unknown", "command": "cargo test"}], "status": "declined"}),
        );
        let shown = card(&item).unwrap();
        assert_eq!((shown.kind, shown.status), (ToolKind::Execute, ToolStatus::Failed));
    }

    #[test]
    fn a_new_file_is_a_write_and_an_update_a_diff() {
        let add = FileChange {
            path: "new.txt".into(),
            kind: json!({"type": "add"}),
            diff: "hello\n".into(),
        };
        let card = file_change(std::slice::from_ref(&add), "inProgress");
        assert!(matches!(&card.view, ToolView::Write { content, .. } if content == "hello\n"));
        let update = FileChange {
            path: "a.txt".into(),
            kind: json!({"type": "update", "move_path": null}),
            diff: "@@ -1 +1 @@\n-a\n+b\n".into(),
        };
        let card = file_change(std::slice::from_ref(&update), "completed");
        let Some(ToolOutput::Diff { hunks, .. }) = &card.output else {
            panic!("{card:?}")
        };
        assert_eq!(hunks[0].lines, ["-a", "+b"]);
        let card = file_change(&[add, update], "completed");
        assert_eq!(card.title, "new.txt, a.txt");
        assert!(matches!(card.output, Some(ToolOutput::Text(_))));
    }

    #[test]
    fn an_mcp_call_shows_its_text_or_its_error() {
        let item = read(
            json!({"type": "mcpToolCall", "id": "m", "server": "docs", "tool": "search",
            "arguments": {"q": "x"}, "status": "completed",
            "result": {"content": [{"type": "text", "text": "found"}]}}),
        );
        let shown = card(&item).unwrap();
        assert_eq!(shown.output, Some(ToolOutput::Text("found".into())));
        let item = read(
            json!({"type": "mcpToolCall", "id": "m", "server": "docs", "tool": "search",
            "arguments": {}, "status": "failed", "error": {"message": "offline"}}),
        );
        assert_eq!(card(&item).unwrap().output, Some(ToolOutput::Text("offline".into())));
    }
}
