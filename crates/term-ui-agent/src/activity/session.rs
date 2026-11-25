//! A session's own lists, which the buttons in its head open: its subagents, or its background processes, each with
//! what runs above what ended.

use std::time::{Duration, SystemTime};

use objc2_app_kit::NSColor;
use splat_i18n::{t, tf};
use term_thread::{BackgroundItem, Subagent, TaskStatus, Thread};

use super::{Action, Line, background_symbol, task_status};
use crate::AgentKey;
use crate::format::{elapsed, tokens};
use crate::work::Part;

/// The lines of `part` of the session in tab `key`, what runs first.
pub(super) fn lines(key: AgentKey, thread: &Thread, part: Part, now: SystemTime) -> Vec<Line> {
    let mut lines: Vec<(bool, Line)> = Vec::new();
    if part == Part::Subagents {
        for sub in thread.subagents() {
            lines.push((sub.status.is_final(), subagent_line(key, sub, now, false)));
        }
    }
    for item in thread.background().iter().filter(|b| part.holds(&b.kind)) {
        lines.push((item.status.is_final(), background_line(key, item, now, false)));
    }
    lines.sort_by_key(|(ended, _)| *ended);
    lines.into_iter().map(|(_, line)| line).collect()
}

/// A subagent's line: its sort, how it stands, how long it ran, its tokens and its last tool.
pub(super) fn subagent_line(key: AgentKey, sub: &Subagent, now: SystemTime, indent: bool) -> Line {
    let ran = match (sub.status.is_final(), sub.ended) {
        (false, _) => now.duration_since(sub.started).unwrap_or_default(),
        (true, Some(ended)) => ended.duration_since(sub.started).unwrap_or_default(),
        (true, None) => Duration::ZERO,
    };
    let mut parts: Vec<String> = Vec::new();
    parts.extend(sub.agent_type.clone());
    if sub.status != TaskStatus::Running {
        parts.push(task_status(sub.status).to_string());
    }
    parts.push(elapsed(ran.max(sub.elapsed)));
    if sub.tokens > 0 {
        parts.push(tf("{} Tokens", &[&tokens(sub.tokens)]));
    }
    if let Some(tool) = &sub.last_tool {
        parts.push(tf("zuletzt {}", &[tool]));
    }
    let mut buttons = vec![(t("Zeigen"), Action::Reveal(key, sub.key.clone()))];
    if let Some(task) = sub
        .task_id
        .clone()
        .filter(|_| sub.cancellable && !sub.status.is_final())
    {
        buttons.push((t("Stopp"), Action::StopTask(key, task)));
    }
    Line {
        symbol: "person.2",
        tint: if sub.status.is_final() {
            NSColor::tertiaryLabelColor()
        } else {
            NSColor::secondaryLabelColor()
        },
        title: sub.description.clone(),
        detail: parts.join(" · "),
        indent,
        buttons,
    }
}

/// A background item's line: how it stands, how long it ran and its last line of output.
pub(super) fn background_line(key: AgentKey, item: &BackgroundItem, now: SystemTime, indent: bool) -> Line {
    let ended = item.status.is_final();
    let mut parts = vec![task_status(item.status).to_string()];
    let until = if ended { item.ended } else { Some(now) };
    if let Some((started, until)) = item.started.zip(until) {
        parts.push(elapsed(until.duration_since(started).unwrap_or_default()));
    }
    if let Some(last) = item
        .output
        .text
        .as_deref()
        .and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()))
    {
        parts.push(last.trim().to_string());
    }
    let mut buttons = vec![(t("Ausgabe zeigen"), Action::ShowOutput(key, item.id.clone()))];
    if !ended {
        buttons.push((t("Stopp"), Action::StopTask(key, item.id.clone())));
    }
    Line {
        symbol: background_symbol(&item.kind),
        tint: if item.ambient || ended {
            NSColor::tertiaryLabelColor()
        } else {
            NSColor::secondaryLabelColor()
        },
        title: item.label.clone(),
        detail: parts.join(" · "),
        indent,
        buttons,
    }
}
