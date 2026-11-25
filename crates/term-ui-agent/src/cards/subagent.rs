//! A subagent's card: what it is to do, how far it got and what it used, and its own timeline once opened.

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSColor, NSView};
use splat_i18n::{t, tf};
use term_thread::{Entry, EntryId, EntryKind, Lane, Subagent, TaskStatus, Thread, ToolCall, ToolView};

use crate::common::util::{activate, pin_edges};

use super::super::format;
use super::super::markdown::Tone;
use super::super::style;
use super::super::ui;
use super::{Ctx, Part, build, disclosure, head, markdown_text, output_text, pack};

/// How many entries of a subagent's timeline an open card shows, the latest.
const SHOWN_ENTRIES: usize = 40;
/// How far a subagent's timeline is set in.
const INDENT: f64 = 14.0;

pub(crate) fn status_text(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => t("Wartet"),
        TaskStatus::Running => t("Läuft"),
        TaskStatus::Paused => t("Pausiert"),
        TaskStatus::Completed => t("Fertig"),
        TaskStatus::Failed => t("Fehlgeschlagen"),
        TaskStatus::Stopped => t("Gestoppt"),
    }
}

/// The facts of a subagent on one line: status, running time, tokens, tool calls and the last tool.
pub(super) fn facts(ctx: &Ctx<'_>, sub: &Subagent) -> String {
    let mut parts = vec![status_text(sub.status).to_string()];
    let elapsed = if sub.status == TaskStatus::Running {
        super::since(ctx, sub.started).max(sub.elapsed)
    } else {
        sub.elapsed
    };
    parts.push(format::elapsed(elapsed));
    if sub.tokens > 0 {
        parts.push(tf("{} Tokens", &[&format::tokens(sub.tokens)]));
    }
    if sub.tool_uses > 0 {
        parts.push(tf("{} Werkzeugaufrufe", &[&sub.tool_uses.to_string()]));
    }
    if let Some(tool) = &sub.last_tool {
        parts.push(tf("zuletzt {}", &[tool]));
    }
    parts.join(" · ")
}

pub(super) fn card(ctx: &Ctx<'_>, id: EntryId, call: &ToolCall) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let ToolView::Agent {
        description,
        agent_type,
        ..
    } = &call.view
    else {
        return ui::view(&ui::label(
            mtm,
            &call.title,
            &style::chat_body(),
            &NSColor::labelColor(),
        ));
    };
    let sub = ctx.thread.subagent(&call.call_id);
    let name = agent_type.clone().unwrap_or_else(|| call.name.clone());
    let running = sub.is_some_and(|s| s.status == TaskStatus::Running);
    let mut trailing: Vec<Retained<NSView>> = Vec::new();
    if running {
        trailing.push(ui::view(&ui::spinner(mtm)));
    }
    if sub.is_some_and(|s| s.status == TaskStatus::Running && s.task_id.is_some() && s.cancellable) {
        trailing.push(ui::view(&ui::button(
            mtm,
            t("Stopp"),
            ctx.target,
            sel!(ideAgentStopTask:),
            pack(id, 0),
        )));
    }
    let refs: Vec<&NSView> = trailing.iter().map(|v| &**v).collect();
    let mut parts: Vec<Retained<NSView>> = vec![ui::view(&head(ctx, &name, description, &refs))];
    if let Some(sub) = sub {
        parts.push(ui::view(&facts_line(ctx, sub)));
        if let Some(summary) = sub.summary.as_ref().filter(|_| sub.status == TaskStatus::Running) {
            parts.push(ui::view(&ui::wrapping(
                mtm,
                summary,
                &style::chat_small(),
                &NSColor::secondaryLabelColor(),
                ctx.width,
            )));
        }
    }
    let lane = Lane::Subagent(call.call_id.clone());
    let count = ctx.thread.entries(&lane).len();
    if count > 0 {
        let open = ctx.is_open(id, Part::Subagent);
        let chevron = disclosure(ctx, id, Part::Subagent);
        let title = tf("Verlauf · {} Einträge", &[&count.to_string()]);
        let link = ui::link(
            mtm,
            &title,
            ctx.target,
            sel!(ideAgentToggle:),
            pack(id, Part::Subagent as u8),
        );
        let spacer = ui::spacer(mtm);
        parts.push(ui::view(&ui::row(mtm, 2.0, &[&chevron, &link, &spacer])));
        if open {
            parts.push(nested(ctx, &lane));
        } else {
            let mut asks = Vec::new();
            waiting(ctx.thread, &lane, 0, &mut asks);
            let inner = Ctx {
                nested: true,
                ..ctx.narrower(0.0)
            };
            parts.extend(asks.into_iter().map(|entry| build(&inner, entry)));
        }
    }
    let finished = sub.filter(|s| s.status.is_final()).and_then(|s| s.summary.clone());
    let report = finished.or_else(|| call.output.as_ref().map(output_text).map(|o| report_of(&o)));
    // The opened timeline ends with the same report.
    let shown = count == 0 || !ctx.is_open(id, Part::Subagent);
    if let Some(report) = report.filter(|r| shown && !r.trim().is_empty()) {
        parts.push(ui::view(&markdown_text(ctx, &report, Tone::Primary)));
    }
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    ui::view(&ui::column(mtm, 5.0, &refs))
}

/// How deep subagents started by subagents are searched for requests.
const NESTING: usize = 8;

/// The requests in `lane` and in the timelines of the subagents it started that wait for the user, in order; a folded
/// card shows them so they can be answered without opening it.
fn waiting<'a>(thread: &'a Thread, lane: &Lane, depth: usize, out: &mut Vec<&'a Entry>) {
    for entry in thread.entries(lane) {
        if entry.kind.waits_for_answer() {
            out.push(entry);
        }
        if depth < NESTING
            && let EntryKind::ToolCall(call) = &entry.kind
            && matches!(call.view, ToolView::Agent { .. })
        {
            waiting(thread, &Lane::Subagent(call.call_id.clone()), depth + 1, out);
        }
    }
}

/// The subagent's own report in a tool result, without the frame the CLI wraps it in for the model.
fn report_of(output: &str) -> String {
    let body = output
        .find("The report follows:")
        .map_or(output, |at| &output[at + "The report follows:".len()..]);
    let body = body.split("agentId:").next().unwrap_or(body);
    body.split("<usage>").next().unwrap_or(body).trim().to_string()
}

fn facts_line(ctx: &Ctx<'_>, sub: &Subagent) -> Retained<NSView> {
    let color = match sub.status {
        TaskStatus::Failed => NSColor::systemRedColor(),
        TaskStatus::Stopped => NSColor::systemOrangeColor(),
        _ => NSColor::secondaryLabelColor(),
    };
    ui::view(&ui::label(ctx.mtm, &facts(ctx, sub), &style::chat_small(), &color))
}

/// The subagent's own timeline, set in behind a line, its latest entries only.
fn nested(ctx: &Ctx<'_>, lane: &Lane) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let inner = Ctx {
        nested: true,
        ..ctx.narrower(INDENT + 2.0)
    };
    let entries = ctx.thread.entries(lane);
    let skipped = entries.len().saturating_sub(SHOWN_ENTRIES);
    let mut cards: Vec<Retained<NSView>> = Vec::new();
    if skipped > 0 {
        cards.push(ui::view(&ui::label(
            mtm,
            &tf("{} frühere Einträge ausgeblendet", &[&skipped.to_string()]),
            &style::chat_small(),
            &NSColor::tertiaryLabelColor(),
        )));
    }
    for entry in &entries[skipped..] {
        cards.push(build(&inner, entry));
    }
    let refs: Vec<&NSView> = cards.iter().map(|v| &**v).collect();
    let column = ui::column(mtm, 8.0, &refs);
    let bar = ui::framed(
        mtm,
        &column,
        &NSColor::clearColor(),
        Some(&NSColor::clearColor()),
        0.0,
        ui::insets(2.0, INDENT, 2.0, 0.0),
    );
    let line = ui::separator(mtm);
    let holder = NSView::new(mtm);
    holder.setTranslatesAutoresizingMaskIntoConstraints(false);
    holder.addSubview(&bar);
    holder.addSubview(&line);
    line.setBoxType(objc2_app_kit::NSBoxType::Custom);
    line.setFillColor(&NSColor::separatorColor());
    line.setBorderWidth(0.0);
    pin_edges(&bar, &holder);
    activate([
        line.leadingAnchor()
            .constraintEqualToAnchor_constant(&holder.leadingAnchor(), 3.0),
        line.widthAnchor().constraintEqualToConstant(2.0),
        line.topAnchor().constraintEqualToAnchor(&holder.topAnchor()),
        line.bottomAnchor().constraintEqualToAnchor(&holder.bottomAnchor()),
    ]);
    holder
}
