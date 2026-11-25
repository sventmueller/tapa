//! Subagents and background tasks: their start, progress, end and the set the CLI runs.

use std::time::{Duration, SystemTime};

use term_thread::{
    BackgroundItem, BackgroundKind, BackgroundPatch, OutputTail, SubagentPatch, TaskStatus, ToolPatch, Update,
};

use super::{Mapper, Task, epoch_millis};
use crate::control::TaskOutput;
use crate::protocol::{BackgroundTask, TaskNotification, TaskProgress, TaskStarted, TaskUpdated, TaskUsage};
use crate::tools;

/// The end of a task as a user-role message tells it: the transcript keeps a background task's end only so, without
/// the `task_notification` frame the live stream has, and the CLI echoes it so too.
pub(super) fn notification_in(text: &str) -> Option<TaskNotification> {
    let body = text.trim_start().strip_prefix("<task-notification>")?;
    let field = |within: &str, name: &str| {
        let (open, close) = (format!("<{name}>"), format!("</{name}>"));
        let start = within.find(&open)? + open.len();
        let end = within[start..].find(&close)? + start;
        Some(within[start..end].trim().to_string())
    };
    // The result, which comes before the counts, is the agent's own text and may quote anything.
    let head = &body[..body.find("<result>").unwrap_or(body.len())];
    let usage = body.rfind("<usage>").map(|at| &body[at..]);
    let count = |name: &str| usage.and_then(|u| field(u, name)).and_then(|v| v.parse::<u64>().ok());
    Some(TaskNotification {
        task_id: field(head, "task-id")?,
        run_id: None,
        tool_use_id: field(head, "tool-use-id"),
        status: field(head, "status").unwrap_or_default(),
        output_file: field(head, "output-file"),
        summary: field(head, "summary"),
        usage: count("subagent_tokens").map(|total_tokens| TaskUsage {
            total_tokens,
            tool_uses: count("tool_uses").unwrap_or(0),
            duration_ms: count("duration_ms").unwrap_or(0),
        }),
    })
}

pub(super) fn task_status(status: &str) -> Option<TaskStatus> {
    Some(match status {
        "pending" => TaskStatus::Pending,
        "running" => TaskStatus::Running,
        "paused" => TaskStatus::Paused,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "killed" | "stopped" => TaskStatus::Stopped,
        _ => return None,
    })
}

fn background_kind(task_type: &str) -> BackgroundKind {
    match task_type {
        "local_bash" => BackgroundKind::Shell,
        "local_agent" => BackgroundKind::Subagent,
        t if t.contains("monitor") => BackgroundKind::Monitor,
        t if t.contains("workflow") => BackgroundKind::Workflow,
        other => BackgroundKind::Other(other.to_string()),
    }
}

impl Mapper {
    /// The updates for the output of a background task that `get_task_output` returned.
    pub(crate) fn task_output(&self, task_id: &str, output: &TaskOutput) -> Vec<Update> {
        vec![Update::BackgroundItem(BackgroundPatch {
            id: task_id.to_string(),
            output: Some(OutputTail {
                path: None,
                text: Some(output.output.clone()),
                total_bytes: Some(output.total_bytes),
                truncated: output.truncated,
            }),
            ..BackgroundPatch::default()
        })]
    }

    /// Whether a frame of `run` of `task` is late because a later run of the task is already known.
    fn late(&mut self, task: &str, run: Option<&String>) -> bool {
        let Some(run) = run else { return false };
        match self.runs.get(task) {
            Some(known) if run < known => true,
            Some(known) if run == known => false,
            _ => {
                self.runs.insert(task.to_string(), run.clone());
                false
            }
        }
    }

    pub(super) fn task_started(&mut self, t: &TaskStarted, now: SystemTime) -> Vec<Update> {
        if self.late(&t.task_id, t.run_id.as_ref()) {
            return Vec::new();
        }
        let kind = t.task_type.as_deref().unwrap_or_default();
        let is_agent = kind == "local_agent" || kind.is_empty() && t.subagent_type.is_some();
        let key = is_agent.then(|| t.tool_use_id.clone().unwrap_or_else(|| t.task_id.clone()));
        let parent = t
            .parent_task_id
            .as_ref()
            .and_then(|p| self.tasks.get(p))
            .and_then(|task| task.subagent.clone());
        self.tasks.insert(
            t.task_id.clone(),
            Task {
                tool_use_id: t.tool_use_id.clone(),
                subagent: key.clone(),
                started: now,
                output_file: None,
            },
        );
        let mut out = Vec::new();
        if let Some(key) = key {
            self.live_subagents.insert(key.clone());
            let mut patch = SubagentPatch::new(key, now);
            patch.task_id = Some(t.task_id.clone());
            patch.agent_type.clone_from(&t.subagent_type);
            patch.description = Some(t.description.clone()).filter(|d| !d.is_empty());
            patch.status = Some(TaskStatus::Running);
            patch.backgrounded = t.is_backgrounded;
            patch.cancellable = Some(true);
            patch.parent = parent;
            out.push(Update::Subagent(patch));
        } else if let (Some(call), Some(true)) = (&t.tool_use_id, t.is_backgrounded) {
            out.push(Update::Tool {
                key: call.clone(),
                patch: ToolPatch {
                    background_task: Some(t.task_id.clone()),
                    ..ToolPatch::default()
                },
            });
        }
        if self.level.iter().any(|b| b.task_id == t.task_id) {
            out.push(Update::Background(self.background_items(now)));
        }
        out
    }

    fn subagent_key(&self, task_id: &str, tool_use_id: Option<&String>) -> Option<String> {
        if let Some(task) = self.tasks.get(task_id) {
            return task.subagent.clone();
        }
        tool_use_id
            .filter(|call| self.tools.get(*call).is_some_and(|t| tools::starts_subagent(&t.name)))
            .cloned()
    }

    pub(super) fn task_progress(&mut self, p: &TaskProgress, now: SystemTime) -> Vec<Update> {
        if self.late(&p.task_id, p.run_id.as_ref()) {
            return Vec::new();
        }
        let Some(key) = self.subagent_key(&p.task_id, p.tool_use_id.as_ref()) else {
            return Vec::new();
        };
        let mut patch = SubagentPatch::new(key, now);
        patch.task_id = Some(p.task_id.clone());
        patch.elapsed = Some(Duration::from_millis(p.usage.duration_ms));
        patch.tokens = Some(p.usage.total_tokens);
        patch.tool_uses = Some(p.usage.tool_uses);
        patch.last_tool.clone_from(&p.last_tool_name);
        patch.summary.clone_from(&p.summary);
        vec![Update::Subagent(patch)]
    }

    pub(super) fn task_updated(&mut self, u: &TaskUpdated, now: SystemTime) -> Vec<Update> {
        if self.late(&u.task_id, u.run_id.as_ref()) {
            return Vec::new();
        }
        let status = u.patch.status.as_deref().and_then(task_status);
        let ended = u.patch.end_time.and_then(epoch_millis);
        match self.subagent_key(&u.task_id, None) {
            Some(key) => {
                if status.is_some_and(TaskStatus::is_final) {
                    self.live_subagents.remove(&key);
                }
                let mut patch = SubagentPatch::new(key, now);
                patch.status = status;
                patch.ended = ended;
                patch.backgrounded = u.patch.is_backgrounded;
                patch.summary.clone_from(&u.patch.error);
                patch.description.clone_from(&u.patch.description);
                if status.is_some_and(TaskStatus::is_final) {
                    patch.cancellable = Some(false);
                }
                vec![Update::Subagent(patch)]
            }
            None => vec![Update::BackgroundItem(BackgroundPatch {
                id: u.task_id.clone(),
                status,
                ended,
                output: None,
                is_backgrounded: u.patch.is_backgrounded,
            })],
        }
    }

    pub(super) fn task_notification(&mut self, n: &TaskNotification, now: SystemTime) -> Vec<Update> {
        if self.late(&n.task_id, n.run_id.as_ref()) {
            return Vec::new();
        }
        let status = task_status(&n.status);
        if let Some(task) = self.tasks.get_mut(&n.task_id) {
            task.output_file.clone_from(&n.output_file);
        }
        match self.subagent_key(&n.task_id, n.tool_use_id.as_ref()) {
            Some(key) => {
                self.live_subagents.remove(&key);
                let mut patch = SubagentPatch::new(key, now);
                patch.task_id = Some(n.task_id.clone());
                patch.status = status;
                patch.ended = Some(now);
                patch.summary.clone_from(&n.summary);
                patch.cancellable = Some(false);
                if let Some(usage) = n.usage {
                    patch.tokens = Some(usage.total_tokens);
                    patch.tool_uses = Some(usage.tool_uses);
                    patch.elapsed = Some(Duration::from_millis(usage.duration_ms));
                }
                vec![Update::Subagent(patch)]
            }
            None => vec![Update::BackgroundItem(BackgroundPatch {
                id: n.task_id.clone(),
                status,
                ended: status.filter(|s| s.is_final()).map(|_| now),
                output: Some(OutputTail {
                    path: n.output_file.clone(),
                    ..OutputTail::default()
                }),
                is_backgrounded: None,
            })],
        }
    }

    pub(super) fn background_changed(&mut self, tasks: &[BackgroundTask], now: SystemTime) -> Vec<Update> {
        // A set that names an older run of a task is older than the set already shown.
        let late = tasks.iter().any(|t| {
            let known = self.runs.get(&t.task_id);
            matches!((t.run_id.as_ref(), known), (Some(run), Some(known)) if run < known)
        });
        if late {
            return Vec::new();
        }
        for task in tasks {
            self.late(&task.task_id, task.run_id.as_ref());
            self.seen.entry(task.task_id.clone()).or_insert(now);
        }
        self.level = tasks.to_vec();
        vec![Update::Background(self.background_items(now))]
    }

    pub(super) fn background_items(&self, now: SystemTime) -> Vec<BackgroundItem> {
        self.level
            .iter()
            .map(|b| {
                let task = self.tasks.get(&b.task_id);
                BackgroundItem {
                    id: b.task_id.clone(),
                    kind: background_kind(&b.task_type),
                    label: b.description.clone(),
                    status: TaskStatus::Running,
                    started: task
                        .map(|t| t.started)
                        .or_else(|| self.seen.get(&b.task_id).copied())
                        .or(Some(now)),
                    ended: None,
                    tool_call: task.and_then(|t| t.tool_use_id.clone()),
                    parent: b.parent_task_id.clone(),
                    output: OutputTail {
                        path: task.and_then(|t| t.output_file.clone()),
                        ..OutputTail::default()
                    },
                    is_backgrounded: true,
                    ambient: b.ambient.unwrap_or(false),
                }
            })
            .collect()
    }
}
