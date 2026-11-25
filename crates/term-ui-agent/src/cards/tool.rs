//! Tool cards: the tool's name and what it does, its input and output, and how far it got.

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{
    NSColor, NSLayoutAttribute, NSMutableParagraphStyle, NSTextBlock, NSTextBlockValueType, NSView, NSWritingDirection,
};
use objc2_foundation::{NSArray, NSAttributedString, NSMutableAttributedString};
use splat_i18n::{t, tf};
use term_thread::{EntryId, Hunk, TextEdit, ToolCall, ToolOutput, ToolStatus, ToolView};

use super::super::attrs::{Attribute, attributed};
use super::super::format;
use super::super::style;
use super::super::text::CardText;
use super::super::ui;
use super::{Ctx, Part, disclosure, head, mono_text, output_text, pack};

/// How many lines of output a closed card shows.
const PREVIEW_LINES: usize = 4;
/// How many lines of a diff a closed card shows.
const DIFF_LINES: usize = 12;
/// How many lines an open card shows at most; the timeline keeps no more once a call has ended.
const MAX_LINES: usize = term_thread::OUTPUT_LINES;
/// How long a line may be before it is cut.
const MAX_LINE: usize = term_thread::OUTPUT_LINE;
/// The width of the IN and OUT labels.
const LABEL_WIDTH: f64 = 28.0;

pub(super) fn card(ctx: &Ctx<'_>, id: EntryId, call: &ToolCall) -> Retained<NSView> {
    match &call.view {
        ToolView::Agent { .. } => super::subagent::card(ctx, id, call),
        ToolView::Command {
            command,
            description,
            background,
            ..
        } => command_card(ctx, id, call, command, description.as_deref(), *background),
        ToolView::Read { path, .. } => read_card(ctx, id, call, path),
        ToolView::Edit { path, edits } => diff_card(ctx, id, call, path, &edit_lines(edits)),
        ToolView::Write { path, content } => diff_card(ctx, id, call, path, &write_lines(content)),
        ToolView::Search {
            pattern,
            path,
            glob,
            results,
        } => {
            let mut detail = tf("„{}“", &[pattern]);
            if let Some(path) = path {
                detail.push_str(&tf(" in {}", &[&format::short_path(path, ctx.cwd)]));
            }
            if let Some(glob) = glob {
                detail.push_str(&format!(" ({glob})"));
            }
            let count = results.map(|n| tf("{} Treffer", &[&n.to_string()]));
            compact_card(ctx, id, call, &detail, count.as_deref(), None)
        }
        ToolView::Fetch { url, prompt } => compact_card(ctx, id, call, url, None, prompt.as_deref()),
        ToolView::WebSearch { query } => compact_card(ctx, id, call, &tf("„{}“", &[query]), None, None),
        ToolView::Mcp { server, tool } => {
            let name = tf("{} · {}", &[server, tool]);
            io_card(ctx, id, call, &name, "", &compact_json(&call.input))
        }
        ToolView::Other => {
            let detail = if call.title == call.name {
                String::new()
            } else {
                call.title.clone()
            };
            io_card(ctx, id, call, &call.name, &detail, &compact_json(&call.input))
        }
    }
}

/// How far the call got, for the end of its head line.
fn status(ctx: &Ctx<'_>, call: &ToolCall) -> Vec<Retained<NSView>> {
    let mtm = ctx.mtm;
    let small = |text: &str, color: &NSColor| {
        let label = ui::label(mtm, text, &style::chat_small(), color);
        ui::hug(&label);
        ui::view(&label)
    };
    match call.status {
        ToolStatus::InProgress => {
            let spinner = ui::spinner(mtm);
            let since = call.started.unwrap_or(ctx.now);
            vec![ui::view(&spinner), ui::view(&ctx.running_label(since))]
        }
        ToolStatus::Pending => vec![small(t("wartet"), &NSColor::tertiaryLabelColor())],
        ToolStatus::Completed => call
            .elapsed
            .map(|e| vec![small(&format::elapsed(e), &NSColor::tertiaryLabelColor())])
            .unwrap_or_default(),
        ToolStatus::Failed => {
            let mut views = vec![small(t("Fehlgeschlagen"), &NSColor::systemRedColor())];
            if let Some(e) = call.elapsed {
                views.push(small(&format::elapsed(e), &NSColor::tertiaryLabelColor()));
            }
            views
        }
    }
}

fn head_with_status(ctx: &Ctx<'_>, call: &ToolCall, name: &str, detail: &str, extra: &[&NSView]) -> Retained<NSView> {
    let status = status(ctx, call);
    let mut trailing: Vec<&NSView> = extra.to_vec();
    trailing.extend(status.iter().map(|v| &**v));
    // An ACP agent that names no tool has the title as the name, which the detail would only repeat.
    let detail = if detail == name { "" } else { detail };
    ui::view(&head(ctx, name, detail, &trailing))
}

/// A shell command: what it is for, then the command and its output in a box.
fn command_card(
    ctx: &Ctx<'_>,
    id: EntryId,
    call: &ToolCall,
    command: &str,
    description: Option<&str>,
    background: bool,
) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let detail = description.unwrap_or(&call.title);
    let badge = (background || call.background_task.is_some()).then(|| {
        let label = ui::label(
            mtm,
            t("Hintergrund"),
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
        );
        ui::hug(&label);
        ui::view(&ui::framed(
            mtm,
            &label,
            &NSColor::separatorColor(),
            None,
            4.0,
            ui::insets(1.0, 5.0, 2.0, 5.0),
        ))
    });
    let extra: Vec<&NSView> = badge.iter().map(|v| &**v).collect();
    let mut parts = vec![head_with_status(ctx, call, &call.name, detail, &extra)];
    let output = call.output.as_ref().map(output_text);
    if !command.is_empty() || output.is_some() {
        parts.push(io_box(ctx, id, command, output.as_deref(), call));
    }
    let mut actions: Vec<Retained<NSView>> = Vec::new();
    if call.status == ToolStatus::InProgress && !background && call.background_task.is_none() && !ctx.nested {
        actions.push(ui::view(&ui::link(
            mtm,
            t("Im Hintergrund ausführen"),
            ctx.target,
            sel!(ideAgentBackground:),
            pack(id, 0),
        )));
    }
    if call.background_task.is_some() {
        actions.push(ui::view(&ui::link(
            mtm,
            t("Ausgabe zeigen"),
            ctx.target,
            sel!(ideAgentShowOutput:),
            pack(id, 0),
        )));
    }
    if !actions.is_empty() {
        actions.push(ui::spacer(mtm));
        let refs: Vec<&NSView> = actions.iter().map(|v| &**v).collect();
        parts.push(ui::view(&ui::row(mtm, 10.0, &refs)));
    }
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    ui::view(&ui::column(mtm, 6.0, &refs))
}

/// A tool whose input is shown as such: an MCP tool or one the app knows no view for.
fn io_card(ctx: &Ctx<'_>, id: EntryId, call: &ToolCall, name: &str, detail: &str, input: &str) -> Retained<NSView> {
    let head = head_with_status(ctx, call, name, detail, &[]);
    let output = call.output.as_ref().map(output_text);
    if input.is_empty() && output.is_none() {
        return head;
    }
    let body = io_box(ctx, id, input, output.as_deref(), call);
    ui::view(&ui::column(ctx.mtm, 6.0, &[&head, &body]))
}

/// The box with the IN row and, once there is output, the OUT row.
fn io_box(ctx: &Ctx<'_>, id: EntryId, input: &str, output: Option<&str>, call: &ToolCall) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let inner = ctx.width - 2.0 - 16.0 - style::chat(LABEL_WIDTH) - 8.0;
    let mut rows: Vec<Retained<NSView>> = Vec::new();
    if !input.is_empty() {
        rows.push(io_row(ctx, "IN", &input_view(ctx, id, input, inner)));
    }
    if let Some(output) = output {
        if !rows.is_empty() {
            rows.push(ui::view(&ui::separator(mtm)));
        }
        let failed = call.status == ToolStatus::Failed;
        let output = output_view(ctx, id, output, call.lines_left_out(), inner, failed);
        rows.push(io_row(ctx, "OUT", &output));
    }
    let refs: Vec<&NSView> = rows.iter().map(|v| &**v).collect();
    let column = ui::column(mtm, 6.0, &refs);
    ui::view(&ui::framed(
        mtm,
        &column,
        &NSColor::separatorColor(),
        Some(&style::code_background()),
        6.0,
        ui::insets(6.0, 8.0, 6.0, 8.0),
    ))
}

fn io_row(ctx: &Ctx<'_>, name: &str, content: &NSView) -> Retained<NSView> {
    let label = ui::label(
        ctx.mtm,
        name,
        &style::mono(style::chat(9.5)),
        &NSColor::secondaryLabelColor(),
    );
    ui::fixed_width(&label, style::chat(LABEL_WIDTH));
    let line = ui::row(ctx.mtm, 8.0, &[&label, content]);
    line.setAlignment(NSLayoutAttribute::Top);
    // Code reads left to right in any interface, so the box keeps that order too.
    line.setUserInterfaceLayoutDirection(objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight);
    ui::view(&line)
}

/// The input on one line, cut; the whole of it once opened.
fn input_view(ctx: &Ctx<'_>, id: EntryId, input: &str, width: f64) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let long = input.contains('\n') || input.chars().count() as f64 * 7.3 > width;
    if long && ctx.is_open(id, Part::Input) {
        let text = mono_text(ctx, &clip(input, MAX_LINES, 0), width - 16.0, &NSColor::labelColor());
        let chevron = disclosure(ctx, id, Part::Input);
        let line = ui::row(mtm, 4.0, &[&text, &chevron]);
        line.setAlignment(NSLayoutAttribute::Top);
        line.setUserInterfaceLayoutDirection(objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight);
        return ui::view(&line);
    }
    let first = input.lines().next().unwrap_or_default();
    let label = ui::label(
        mtm,
        first,
        &style::mono(style::chat(style::CODE)),
        &NSColor::labelColor(),
    );
    label.setUserInterfaceLayoutDirection(objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight);
    label.setBaseWritingDirection(NSWritingDirection::LeftToRight);
    label.setSelectable(true);
    if !long {
        return ui::view(&label);
    }
    let chevron = disclosure(ctx, id, Part::Input);
    ui::view(&ui::row(mtm, 4.0, &[&label, &chevron]))
}

/// The first lines of the output and a link to the rest; all of it, up to a limit, once opened.
/// `left_out` lines that the timeline dropped follow the end of `output`.
fn output_view(
    ctx: &Ctx<'_>,
    id: EntryId,
    output: &str,
    left_out: usize,
    width: f64,
    failed: bool,
) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let color = if failed {
        NSColor::systemRedColor()
    } else {
        NSColor::labelColor()
    };
    if output.trim().is_empty() {
        return ui::view(&ui::label(
            mtm,
            t("(keine Ausgabe)"),
            &style::chat_small(),
            &NSColor::tertiaryLabelColor(),
        ));
    }
    let total = output.lines().count() + left_out;
    let open = ctx.is_open(id, Part::Output);
    let shown = if open { MAX_LINES } else { PREVIEW_LINES };
    let text = mono_text(ctx, &clip(output, shown, left_out), width, &color);
    if total <= PREVIEW_LINES {
        return ui::view(&text);
    }
    let more = if open {
        t("Weniger zeigen").to_string()
    } else {
        tf("{} weitere Zeilen", &[&(total - PREVIEW_LINES).to_string()])
    };
    let link = ui::link(
        mtm,
        &more,
        ctx.target,
        sel!(ideAgentToggle:),
        pack(id, Part::Output as u8),
    );
    let spacer = ui::spacer(mtm);
    let line = ui::row(mtm, 0.0, &[&link, &spacer]);
    ui::view(&ui::column(mtm, 2.0, &[&text, &line]))
}

/// The first `lines` lines of `text`, each at most `MAX_LINE` characters, with a note when more were left out,
/// counting `left_out` lines that follow `text`.
fn clip(text: &str, lines: usize, left_out: usize) -> String {
    let mut out: Vec<String> = text
        .lines()
        .take(lines)
        .map(|l| {
            if l.chars().nth(MAX_LINE).is_some() {
                let mut cut: String = l.chars().take(MAX_LINE).collect();
                cut.push('…');
                cut
            } else {
                l.to_string()
            }
        })
        .collect();
    if lines >= MAX_LINES {
        let total = text.lines().count() + left_out;
        if total > lines {
            out.push(tf("… {} weitere Zeilen", &[&(total - lines).to_string()]));
        }
    }
    out.join("\n")
}

/// A tool that reads: one head line, its output behind a chevron.
fn compact_card(
    ctx: &Ctx<'_>,
    id: EntryId,
    call: &ToolCall,
    detail: &str,
    count: Option<&str>,
    note: Option<&str>,
) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let mut extra: Vec<Retained<NSView>> = Vec::new();
    if let Some(count) = count {
        let label = ui::label(mtm, count, &style::chat_small(), &NSColor::secondaryLabelColor());
        ui::hug(&label);
        extra.push(ui::view(&label));
    }
    let output = call.output.as_ref().map(output_text).filter(|o| !o.trim().is_empty());
    if output.is_some() {
        extra.push(ui::view(&disclosure(ctx, id, Part::Output)));
    }
    let refs: Vec<&NSView> = extra.iter().map(|v| &**v).collect();
    let mut parts = vec![head_with_status(ctx, call, &call.name, detail, &refs)];
    if let Some(note) = note {
        parts.push(ui::view(&ui::wrapping(
            mtm,
            note,
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
            ctx.width,
        )));
    }
    if let Some(output) = output.filter(|_| ctx.is_open(id, Part::Output)) {
        let text = mono_text(
            ctx,
            &clip(&output, MAX_LINES, call.lines_left_out()),
            ctx.width - 18.0,
            &NSColor::labelColor(),
        );
        parts.push(ui::view(&ui::framed(
            mtm,
            &text,
            &NSColor::separatorColor(),
            Some(&style::code_background()),
            6.0,
            ui::insets(6.0, 8.0, 6.0, 8.0),
        )));
    }
    if parts.len() == 1 {
        return parts.remove(0);
    }
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    ui::view(&ui::column(mtm, 6.0, &refs))
}

/// A read file: its path and lines, opening in the editor.
fn read_card(ctx: &Ctx<'_>, id: EntryId, call: &ToolCall, path: &str) -> Retained<NSView> {
    let mut detail = format::short_path(path, ctx.cwd);
    if let Some((first, last)) = call.view.line_range() {
        let range = match last {
            Some(last) => tf("Zeilen {}–{}", &[&first.to_string(), &last.to_string()]),
            None => tf("ab Zeile {}", &[&first.to_string()]),
        };
        detail.push_str(&format!(" · {range}"));
    }
    compact_card(ctx, id, call, &detail, None, None)
}

/// A changed file: the change as red and green lines, and a way into the editor.
fn diff_card(ctx: &Ctx<'_>, id: EntryId, call: &ToolCall, path: &str, proposed: &[String]) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let (lines, left_out, (added_out, removed_out)) = match &call.output {
        Some(ToolOutput::Diff { hunks, .. }) => (hunk_lines(hunks), call.lines_left_out(), call.changes_left_out()),
        _ => (proposed.to_vec(), 0, (0, 0)),
    };
    let added = lines.iter().filter(|l| l.starts_with('+')).count() + added_out;
    let removed = lines.iter().filter(|l| l.starts_with('-')).count() + removed_out;
    let counts = ui::label(
        mtm,
        &format!("+{added} −{removed}"),
        &style::mono(style::chat(style::SMALL)),
        &NSColor::secondaryLabelColor(),
    );
    counts.setBaseWritingDirection(NSWritingDirection::LeftToRight);
    ui::hug(&counts);
    let open_file = ui::link(
        mtm,
        t("Im Editor öffnen"),
        ctx.target,
        sel!(ideAgentOpenPath:),
        pack(id, 0),
    );
    ui::hug(&open_file);
    let head = head_with_status(
        ctx,
        call,
        &call.name,
        &format::short_path(path, ctx.cwd),
        &[&counts, &open_file],
    );
    if lines.is_empty() {
        return head;
    }
    let open = ctx.is_open(id, Part::Output);
    let shown: Vec<String> = if open {
        lines.iter().take(MAX_LINES).cloned().collect()
    } else {
        lines.iter().take(DIFF_LINES).cloned().collect()
    };
    let width = ctx.width - 2.0;
    let text = CardText::new(mtm, &diff_text(&shown), width, true);
    text.left_to_right();
    let frame = ui::framed(
        mtm,
        &text,
        &NSColor::separatorColor(),
        None,
        6.0,
        ui::insets(0.0, 0.0, 0.0, 0.0),
    );
    let mut parts = vec![head, ui::view(&frame)];
    let total = lines.len() + left_out;
    if total > DIFF_LINES {
        let title = if open {
            t("Weniger zeigen").to_string()
        } else {
            tf("Alle {} Zeilen zeigen", &[&total.to_string()])
        };
        let link = ui::link(
            mtm,
            &title,
            ctx.target,
            sel!(ideAgentToggle:),
            pack(id, Part::Output as u8),
        );
        let spacer = ui::spacer(mtm);
        parts.push(ui::view(&ui::row(mtm, 0.0, &[&link, &spacer])));
    }
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    ui::view(&ui::column(mtm, 6.0, &refs))
}

fn hunk_lines(hunks: &[Hunk]) -> Vec<String> {
    let mut lines = Vec::new();
    for (n, hunk) in hunks.iter().enumerate() {
        if n > 0 {
            lines.push("⋯".to_string());
        }
        lines.extend(hunk.lines.iter().cloned());
    }
    lines
}

/// The lines an edit would change, before the agent reports the diff.
fn edit_lines(edits: &[TextEdit]) -> Vec<String> {
    edits
        .iter()
        .flat_map(|e| {
            e.old
                .lines()
                .map(|l| format!("-{l}"))
                .chain(e.new.lines().map(|l| format!("+{l}")))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn write_lines(content: &str) -> Vec<String> {
    content.lines().map(|l| format!("+{l}")).collect()
}

/// Diff lines in code type, each on a full-width red or green ground.
fn diff_text(lines: &[String]) -> Retained<NSAttributedString> {
    let out = NSMutableAttributedString::new();
    let font = style::mono(style::chat(style::CODE));
    let count = lines.len();
    for (n, line) in lines.iter().enumerate() {
        let (background, color) = match line.chars().next() {
            Some('+') => (Some(style::added_background()), NSColor::labelColor()),
            Some('-') => (Some(style::removed_background()), NSColor::labelColor()),
            Some('⋯') => (None, NSColor::tertiaryLabelColor()),
            _ => (None, NSColor::secondaryLabelColor()),
        };
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setBaseWritingDirection(NSWritingDirection::LeftToRight);
        let block = NSTextBlock::new();
        block.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
        block.setWidth_type_forLayer(
            8.0,
            NSTextBlockValueType::AbsoluteValueType,
            objc2_app_kit::NSTextBlockLayer::Padding,
        );
        if let Some(background) = &background {
            block.setBackgroundColor(Some(background));
        }
        paragraph.setTextBlocks(&NSArray::from_slice(&[&*block]));
        let text = if n + 1 < count {
            format!("{line}\n")
        } else {
            line.clone()
        };
        out.appendAttributedString(&attributed(
            &text,
            &[
                Attribute::font(&font),
                Attribute::foreground(&color),
                Attribute::paragraph(&paragraph),
            ],
        ));
    }
    Retained::into_super(out)
}

/// The input as compact JSON on as few lines as it takes, for tools without a view of their own.
fn compact_json(input: &serde_json::Value) -> String {
    match input {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Object(map) if map.is_empty() => String::new(),
        serde_json::Value::Object(map) => {
            // Sorted on purpose: whether a JSON object keeps the agent's order depends on a crate feature that the
            // ACP SDK turns on for the whole app.
            let mut fields: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            fields.sort_by_key(|(k, _)| *k);
            fields
                .into_iter()
                .map(|(k, v)| match v {
                    serde_json::Value::String(s) => format!("{k}: {s}"),
                    other => format!("{k}: {other}"),
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_output_is_cut_and_says_how_much_is_left() {
        let text: String = (0..500).map(|n| format!("line {n}\n")).collect();
        let clipped = clip(&text, MAX_LINES, 0);
        assert_eq!(clipped.lines().count(), MAX_LINES + 1);
        assert!(clipped.ends_with("100 weitere Zeilen"));
        assert!(clip(&text, MAX_LINES, 50).ends_with("150 weitere Zeilen"));
        let kept: String = (0..MAX_LINES).map(|n| format!("line {n}\n")).collect();
        assert!(
            clip(&kept, MAX_LINES, 7).ends_with("7 weitere Zeilen"),
            "lines the timeline dropped are counted"
        );
        assert_eq!(clip("a\nb\nc", PREVIEW_LINES, 0), "a\nb\nc");
        assert_eq!(
            clip("a\nb\nc\nd\ne\nf", PREVIEW_LINES, 0),
            "a\nb\nc\nd",
            "a preview names no rest"
        );
        let long = "x".repeat(MAX_LINE + 1);
        assert_eq!(clip(&long, 1, 0).chars().count(), MAX_LINE + 1);
        assert!(clip(&long, 1, 0).ends_with('…'));
        assert_eq!(clip(&long[..MAX_LINE], 1, 0), &long[..MAX_LINE]);
    }

    #[test]
    fn an_edit_shows_as_removed_and_added_lines_before_its_diff_arrives() {
        let edits = [TextEdit {
            old: "world".into(),
            new: "there".into(),
            replace_all: false,
        }];
        assert_eq!(edit_lines(&edits), ["-world", "+there"]);
        assert_eq!(write_lines("a\nb\n"), ["+a", "+b"]);
    }

    #[test]
    fn json_input_reads_as_one_field_per_line() {
        let input = serde_json::json!({"query": "rust", "limit": 3});
        assert_eq!(compact_json(&input), "limit: 3\nquery: rust");
        assert_eq!(compact_json(&serde_json::Value::Null), "");
    }
}
