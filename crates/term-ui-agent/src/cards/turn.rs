//! The conversation itself: what the user sent, the agent's answer, its thinking and its to-do list.

use objc2::rc::Retained;
use objc2::sel;
use objc2_app_kit::{NSButton, NSCellImagePosition, NSColor, NSFont, NSView};
use objc2_foundation::NSMutableAttributedString;
use splat_i18n::{t, tf};
use term_thread::{Attachment, Entry, EntryId, Thought, Todo, TodoStatus, UserMessage};

use super::super::attrs::{Attribute, attributed};
use super::super::format;
use super::super::markdown::Tone;
use super::super::style;
use super::super::text::CardText;
use super::super::ui;
use super::{Ctx, Part, markdown_text, pack};
use crate::common::util::{ns, set_action, symbol};

/// The user's message in a framed box, with its time above and its attachments below.
pub(super) fn user(ctx: &Ctx<'_>, entry: &Entry, message: &UserMessage) -> Retained<NSView> {
    let mtm = ctx.mtm;
    if ctx.nested {
        let text = tf("Auftrag: {}", &[&format::first_line(&message.text, 400)]);
        let label = ui::wrapping(
            mtm,
            &text,
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
            ctx.width,
        );
        return ui::view(&label);
    }
    let inner_width = ctx.width - 2.0 - 20.0;
    let text = CardText::new(mtm, &user_text(&message.text), inner_width, true);
    let mut content: Vec<Retained<NSView>> = vec![ui::view(&text)];
    if !message.attachments.is_empty() {
        content.push(ui::view(&attachment_row(ctx, &message.attachments)));
    }
    let refs: Vec<&NSView> = content.iter().map(|v| &**v).collect();
    let body = ui::column(mtm, 6.0, &refs);
    let frame = ui::framed(
        mtm,
        &body,
        &NSColor::separatorColor(),
        Some(&style::code_background()),
        8.0,
        ui::insets(8.0, 10.0, 8.0, 8.0),
    );
    let time = ui::label(
        mtm,
        &format::time_of_day(entry.at),
        &style::chat_small(),
        &NSColor::tertiaryLabelColor(),
    );
    ui::hug(&time);
    let spacer = ui::spacer(mtm);
    let stamp = ui::row(mtm, 0.0, &[&spacer, &time]);
    ui::column(mtm, 4.0, &[&stamp, &frame]).into_super()
}

/// What the user typed; a leading slash command in code type, as the CLI reads it.
fn user_text(text: &str) -> Retained<objc2_foundation::NSAttributedString> {
    let body = style::chat_body();
    let out = NSMutableAttributedString::new();
    let (command, rest) = match text.strip_prefix('/') {
        Some(after) if !after.starts_with(char::is_whitespace) => {
            let end = text.find(char::is_whitespace).unwrap_or(text.len());
            (&text[..end], &text[end..])
        }
        _ => ("", text),
    };
    if !command.is_empty() {
        out.appendAttributedString(&attributed(
            command,
            &[
                Attribute::font(&style::mono(style::chat(style::BODY))),
                Attribute::foreground(&NSColor::labelColor()),
            ],
        ));
    }
    out.appendAttributedString(&super::super::markdown::plain(rest, &body, &NSColor::labelColor()));
    objc2::rc::Retained::into_super(out)
}

fn attachment_row(ctx: &Ctx<'_>, attachments: &[Attachment]) -> Retained<objc2_app_kit::NSStackView> {
    let chips: Vec<Retained<NSView>> = attachments
        .iter()
        .map(|a| {
            let (name, symbol_name) = match a {
                Attachment::Image { name, data, .. } => {
                    let name = name.clone().unwrap_or_else(|| t("Bild").to_string());
                    let pictures = ctx.inputs.borrow().pictures.clone();
                    return ui::picture_tile(ctx.mtm, data, &name, None, Some(&pictures));
                }
                Attachment::File { path, lines } => {
                    let mut name = format::short_path(&path.to_string_lossy(), ctx.cwd);
                    if let Some((a, b)) = lines {
                        name.push_str(&format!(":{a}-{b}"));
                    }
                    (name, "doc.text")
                }
                Attachment::Selection { path, lines, .. } => (
                    format!(
                        "{}:{}-{}",
                        format::short_path(&path.to_string_lossy(), ctx.cwd),
                        lines.0,
                        lines.1
                    ),
                    "text.cursor",
                ),
                Attachment::External { label, .. } => {
                    return outside_chip(ctx, label);
                }
            };
            chip(ctx, symbol_name, &name)
        })
        .collect();
    let refs: Vec<&NSView> = chips.iter().map(|v| &**v).collect();
    ui::row(ctx.mtm, 4.0, &refs)
}

/// The chip of outside text the message took along, with its mark.
fn outside_chip(ctx: &Ctx<'_>, text: &str) -> Retained<NSView> {
    let icon = ui::icon(
        ctx.mtm,
        "globe",
        text,
        style::chat(10.0),
        &NSColor::secondaryLabelColor(),
    );
    let label = ui::label(ctx.mtm, text, &style::chat_small(), &NSColor::secondaryLabelColor());
    let marker = super::super::outside::marker(ctx.mtm);
    let line = ui::row(ctx.mtm, 3.0, &[&icon, &label, &marker]);
    ui::framed(
        ctx.mtm,
        &line,
        &NSColor::systemOrangeColor().colorWithAlphaComponent(0.6),
        None,
        5.0,
        ui::insets(2.0, 5.0, 2.0, 6.0),
    )
    .into_super()
}

/// A small framed label with a symbol, as attachments show.
pub(super) fn chip(ctx: &Ctx<'_>, symbol_name: &str, text: &str) -> Retained<NSView> {
    let icon = ui::icon(
        ctx.mtm,
        symbol_name,
        text,
        style::chat(10.0),
        &NSColor::secondaryLabelColor(),
    );
    let label = ui::label(ctx.mtm, text, &style::chat_small(), &NSColor::secondaryLabelColor());
    let line = ui::row(ctx.mtm, 3.0, &[&icon, &label]);
    ui::framed(
        ctx.mtm,
        &line,
        &NSColor::separatorColor(),
        None,
        5.0,
        ui::insets(2.0, 5.0, 2.0, 6.0),
    )
    .into_super()
}

/// The width of the copy button beside an answer.
const COPY_WIDTH: f64 = 18.0;

/// The agent's answer as rendered markdown, with a button that copies it once it is complete.
pub(super) fn answer(ctx: &Ctx<'_>, id: EntryId, markdown: &str, streaming: bool) -> Retained<NSView> {
    if markdown.trim().is_empty() && streaming {
        return ui::view(&ui::label(
            ctx.mtm,
            "…",
            &style::chat_body(),
            &NSColor::secondaryLabelColor(),
        ));
    }
    if streaming && !ctx.nested {
        return ui::view(&CardText::streaming(ctx.mtm, markdown, ctx.width));
    }
    if ctx.nested {
        return ui::view(&markdown_text(ctx, markdown, Tone::Primary));
    }
    let text = markdown_text(&ctx.narrower(style::chat(COPY_WIDTH) + 6.0), markdown, Tone::Primary);
    let copy = ui::symbol_button(
        ctx.mtm,
        "doc.on.doc",
        t("Antwort kopieren"),
        style::chat(11.0),
        ctx.target,
        sel!(ideAgentCopy:),
        pack(id, 0),
    );
    ui::fixed_width(&copy, style::chat(COPY_WIDTH));
    let line = ui::row(ctx.mtm, 6.0, &[&text, &copy]);
    line.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
    ui::view(&line)
}

/// "Thinking", closed, with its duration; opens to the thought itself.
pub(super) fn thought(ctx: &Ctx<'_>, id: EntryId, thought: &Thought) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let title = match (thought.streaming, thought.duration) {
        (true, _) => t("Denkt …").to_string(),
        (false, Some(duration)) => tf("Gedacht · {}", &[&format::elapsed(duration)]),
        (false, None) => t("Gedacht").to_string(),
    };
    let has_text = !thought.text.trim().is_empty();
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(&title), None, None, mtm) };
    button.setBordered(false);
    button.setFont(Some(&NSFont::systemFontOfSize(style::chat(style::BODY - 1.0))));
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    let open = has_text && ctx.is_open(id, Part::Thought);
    if has_text {
        set_action(&button, ctx.target, sel!(ideAgentToggle:));
        button.setTag(pack(id, Part::Thought as u8));
        let name = if open { "chevron.down" } else { "chevron.forward" };
        button.setImage(symbol(name, t("Aufklappen")).as_deref());
        button.setImagePosition(NSCellImagePosition::ImageTrailing);
    } else {
        button.setEnabled(false);
    }
    ui::hug(&button);
    let spacer = ui::spacer(mtm);
    let line = ui::row(mtm, 4.0, &[&button, &spacer]);
    if !open {
        return line.into_super();
    }
    let text = markdown_text(ctx, &thought.text, Tone::Secondary);
    ui::column(mtm, 4.0, &[&line, &text]).into_super()
}

/// The agent's to-do list with a check for each done item.
pub(super) fn todos(ctx: &Ctx<'_>, todos: &[Todo]) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let done = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
    let progress = tf("{} von {} erledigt", &[&done.to_string(), &todos.len().to_string()]);
    let title = super::head(ctx, t("To-dos"), &progress, &[]);
    let mut rows: Vec<Retained<NSView>> = vec![title.into_super()];
    for todo in todos {
        let (symbol_name, color, text) = match todo.status {
            TodoStatus::Completed => (
                "checkmark.circle.fill",
                NSColor::systemGreenColor(),
                todo.content.clone(),
            ),
            TodoStatus::InProgress => (
                "circle.lefthalf.filled",
                NSColor::controlAccentColor(),
                todo.active_form.clone().unwrap_or_else(|| todo.content.clone()),
            ),
            TodoStatus::Pending => ("circle", NSColor::tertiaryLabelColor(), todo.content.clone()),
        };
        let icon = ui::icon(mtm, symbol_name, &text, style::chat(12.0), &color);
        let (font, text_color) = match todo.status {
            TodoStatus::InProgress => (style::medium(style::chat(style::BODY)), NSColor::labelColor()),
            TodoStatus::Completed => (style::chat_body(), NSColor::secondaryLabelColor()),
            TodoStatus::Pending => (style::chat_body(), NSColor::labelColor()),
        };
        let label = ui::wrapping(mtm, &text, &font, &text_color, ctx.width - 24.0);
        if todo.status == TodoStatus::Completed {
            label.setAttributedStringValue(&attributed(
                &text,
                &[
                    Attribute::font(&font),
                    Attribute::foreground(&text_color),
                    Attribute::strikethrough(),
                ],
            ));
        }
        let line = ui::row(mtm, 6.0, &[&icon, &label]);
        rows.push(line.into_super());
    }
    let refs: Vec<&NSView> = rows.iter().map(|v| &**v).collect();
    ui::column(mtm, 4.0, &refs).into_super()
}
