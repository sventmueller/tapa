//! What the agent asks the user: permission to run a tool, questions with options, approval of a plan.

use objc2::rc::Retained;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{
    NSAccessibility, NSBorderType, NSButton, NSButtonType, NSColor, NSScrollView, NSStackView, NSTextField, NSTextView,
    NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use serde_json::Value;
use splat_i18n::{t, tf};
use term_thread::{
    Decision, EntryId, EntryKind, Lane, PermissionOption, PermissionRequest, PlanApproval, Question, QuestionItem,
    RequestState, RuleScope, Subagent, ToolView,
};

use super::super::format;
use super::super::markdown::Tone;
use super::super::style;
use super::super::text::CardText;
use super::super::ui;
use super::{Ctx, Part, disclosure, markdown_text, pack};
use crate::common::util::ns;

/// What a permission card's buttons send in their tag beside the entry.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub const ALLOW_ONCE: u8 = 1;
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
/// The tag of allowing always, with the rule the request suggests.
pub const ALLOW_ALWAYS: u8 = 2;
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
/// The tag of refusing.
pub const DENY: u8 = 3;

/// How much of a rule the button that stores it shows; its tooltip shows all of it.
const RULE_LENGTH: usize = 48;

/// What a plan's answer buttons send in their tag beside the entry.
pub(crate) const PLAN_APPROVE: u8 = 1;
/// The tag of the button that sends the plan back with the user's comment.
pub(crate) const PLAN_REVISE: u8 = 2;
/// The tag of the button that rejects the plan.
pub(crate) const PLAN_REJECT: u8 = 3;

/// The text the user edits before allowing: a command as such, other input as JSON.
pub(crate) fn editable_input(tool: &str, input: &Value) -> String {
    match edited_field(tool, input)
        .and_then(|field| input.get(field))
        .and_then(Value::as_str)
    {
        Some(text) => text.to_string(),
        None => serde_json::to_string_pretty(input).unwrap_or_default(),
    }
}

/// The field of `input` the user edits as plain text: the command, the new text of an edit, the content written.
fn edited_field(tool: &str, input: &Value) -> Option<&'static str> {
    let field = match tool {
        "Bash" => "command",
        "Edit" => "new_string",
        "Write" => "content",
        _ => return None,
    };
    input.get(field).and_then(Value::as_str).map(|_| field)
}

/// What the user edits before allowing, as the editor's caption.
pub(crate) fn edited_caption(tool: &str, input: &Value) -> Option<&'static str> {
    match edited_field(tool, input)? {
        "new_string" => Some(t("Ersetzt durch – vor dem Erlauben änderbar")),
        "content" => Some(t("Neuer Inhalt – vor dem Erlauben änderbar")),
        _ => None,
    }
}

/// The input the edited text stands for; `None` when it is not valid.
pub(crate) fn edited_input(tool: &str, original: &Value, text: &str) -> Option<Value> {
    match edited_field(tool, original) {
        Some(field) => {
            let mut input = original.clone();
            input[field] = Value::from(text);
            Some(input)
        }
        None => serde_json::from_str::<Value>(text).ok().filter(Value::is_object),
    }
}

/// How the request's command throws work away, if it runs a destructive git command, which needs a question every
/// time: the command of the input, or for an agent that names it only there, of the title.
pub(crate) fn destructive(request: &PermissionRequest) -> Option<term_thread::shell::Destructive> {
    match term_thread::shell::command_of(&request.input) {
        Some(command) => term_thread::shell::destructive_git(&command),
        None => term_thread::shell::destructive_git(request.title.as_deref()?),
    }
}

/// Where a stored rule applies, as the user reads it.
pub fn scope_name(scope: &RuleScope) -> String {
    match scope {
        RuleScope::Session => t("diese Sitzung").to_string(),
        RuleScope::ProjectLocal => t("dieses Projekt, nur für dich").to_string(),
        RuleScope::Project => t("dieses Projekt").to_string(),
        RuleScope::User => t("alle Projekte").to_string(),
        RuleScope::Other(other) => other.clone(),
    }
}

pub(crate) fn permission(ctx: &Ctx<'_>, id: EntryId, request: &PermissionRequest) -> Retained<NSView> {
    match &request.state {
        RequestState::Pending => pending_permission(ctx, id, request),
        RequestState::Decided(decision) => {
            let (symbol, color, text) = match decision {
                Decision::Allowed { always, edited } => {
                    let mut text = tf("{} erlaubt", &[&request.tool_name]);
                    if *edited {
                        text.push_str(&format!(" · {}", t("bearbeitet")));
                    }
                    if *always {
                        text.push_str(&format!(" · {}", t("Regel gespeichert")));
                    }
                    ("checkmark.shield", NSColor::systemGreenColor(), text)
                }
                Decision::Denied { message } => {
                    let mut text = tf("{} abgelehnt", &[&request.tool_name]);
                    if let Some(message) = message.as_ref().filter(|m| !m.is_empty()) {
                        text.push_str(&format!(" · {message}"));
                    }
                    ("xmark.shield", NSColor::systemRedColor(), text)
                }
                _ => ("shield", NSColor::secondaryLabelColor(), t("Entschieden").to_string()),
            };
            status_line(ctx, symbol, &color, &text)
        }
        RequestState::Withdrawn => status_line(
            ctx,
            "shield.slash",
            &NSColor::tertiaryLabelColor(),
            &tf("Anfrage für {} zurückgezogen", &[&request.tool_name]),
        ),
    }
}

fn status_line(ctx: &Ctx<'_>, symbol: &str, color: &NSColor, text: &str) -> Retained<NSView> {
    let icon = ui::icon(ctx.mtm, symbol, text, style::chat(11.0), color);
    let label = ui::label(ctx.mtm, text, &style::chat_small(), &NSColor::secondaryLabelColor());
    let spacer = ui::spacer(ctx.mtm);
    ui::view(&ui::row(ctx.mtm, 5.0, &[&icon, &label, &spacer]))
}

fn pending_permission(ctx: &Ctx<'_>, id: EntryId, request: &PermissionRequest) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let inner = ctx.width - 2.0 - 24.0;
    let call = request
        .tool_call
        .as_deref()
        .and_then(|key| ctx.thread.entry_by_key(key));
    let subagent = call
        .and_then(|entry| ctx.thread.place(entry.id))
        .and_then(|(lane, _)| match lane {
            Lane::Subagent(owner) => ctx.thread.subagent(owner),
            Lane::Main => None,
        });
    let view = call.and_then(|entry| match &entry.kind {
        EntryKind::ToolCall(call) => Some(&call.view),
        _ => None,
    });
    let mut parts: Vec<Retained<NSView>> = vec![permission_head(ctx, request, subagent, view, inner)];
    let mut note = |text: String| {
        parts.push(ui::view(&ui::wrapping(
            mtm,
            &text,
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
            inner,
        )));
    };
    if let Some(task) = subagent.map(|s| s.description.as_str()).filter(|d| !d.is_empty()) {
        note(tf("Aufgabe: {}", &[task]));
    }
    let subject = view.and_then(|view| subject(ctx, view));
    if let Some(subject) = &subject {
        note(subject.clone());
    }
    let repeats = |text: &str| {
        subject.as_deref().is_some_and(|s| s.contains(text))
            || request.blocked_path.as_deref().is_some_and(|p| p.ends_with(text))
    };
    if let Some(description) = request.description.as_ref().filter(|d| !repeats(d)) {
        note(description.clone());
    }
    let reason = destructive(request).map(destructive_text);
    if let Some(reason) = reason.as_ref().or(request.reason.as_ref()) {
        note(tf("Grund: {}", &[reason]));
    }
    if let Some(path) = request.blocked_path.as_deref().map(|p| format::short_path(p, ctx.cwd))
        && !subject.as_deref().is_some_and(|s| s.ends_with(&path))
    {
        note(tf("Pfad: {}", &[&path]));
    }
    if ctx.thread.agent() == term_claude::AGENT {
        if let Some(ToolView::Edit { edits, .. }) = view
            && let [edit] = edits.as_slice()
        {
            let small = |text: &str| ui::label(mtm, text, &style::chat_small(), &NSColor::secondaryLabelColor());
            parts.push(ui::view(&small(t("Bisher"))));
            let old = edit.old.lines().take(10).collect::<Vec<_>>().join("\n");
            parts.push(ui::view(&super::mono_text(
                ctx,
                &old,
                inner,
                &NSColor::secondaryLabelColor(),
            )));
        }
        if let Some(caption) = edited_caption(&request.tool_name, &request.input) {
            parts.push(ui::view(&ui::label(
                mtm,
                caption,
                &style::chat_small(),
                &NSColor::secondaryLabelColor(),
            )));
        }
        parts.push(ui::view(&editor(ctx, id, request, inner)));
    } else if !request.input.is_null() && request.tool_call.is_none() {
        // ACP answers a permission with one of the agent's options and cannot carry a changed input; the tool's own
        // card shows the input where there is one.
        let text = editable_input(&request.tool_name, &request.input);
        parts.push(ui::view(&super::mono_text(ctx, &text, inner, &NSColor::labelColor())));
    }
    parts.push(permission_buttons(ctx, id, request));
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    let column = ui::column(mtm, 7.0, &refs);
    ui::view(&ui::framed(
        mtm,
        &column,
        &NSColor::systemOrangeColor(),
        None,
        8.0,
        ui::insets(10.0, 12.0, 10.0, 12.0),
    ))
}

/// The card's head: who asks to use which tool, wrapping rather than cut off.
fn permission_head(
    ctx: &Ctx<'_>,
    request: &PermissionRequest,
    subagent: Option<&Subagent>,
    view: Option<&ToolView>,
    inner: f64,
) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let tool = match view {
        Some(ToolView::Mcp { server, tool }) => tf("{} · {}", &[server, tool]),
        _ => request.title.clone().unwrap_or_else(|| request.tool_name.clone()),
    };
    let who = match subagent {
        Some(sub) => tf(
            "Subagent „{}“",
            &[sub.agent_type.as_deref().unwrap_or("general-purpose")],
        ),
        None => crate::agent_name(ctx.thread.agent()).to_string(),
    };
    let icon = ui::icon(
        mtm,
        "exclamationmark.shield",
        t("Freigabe"),
        style::chat(13.0),
        &NSColor::systemOrangeColor(),
    );
    let title = ui::wrapping(
        mtm,
        &tf("{} möchte {} verwenden", &[&who, &tool]),
        &style::bold(style::chat(style::BODY)),
        &NSColor::labelColor(),
        inner - 26.0,
    );
    let spacer = ui::spacer(mtm);
    ui::view(&ui::row(mtm, 6.0, &[&icon, &title, &spacer]))
}

/// What the call works on, where the editor below does not show it: the file, the address, the search.
fn subject(ctx: &Ctx<'_>, view: &ToolView) -> Option<String> {
    match view {
        ToolView::Read { path, .. } | ToolView::Edit { path, .. } | ToolView::Write { path, .. } => {
            Some(tf("Pfad: {}", &[&format::short_path(path, ctx.cwd)]))
        }
        ToolView::Fetch { url, prompt } => Some(match prompt {
            Some(prompt) => format!("{url}\n{prompt}"),
            None => url.clone(),
        }),
        ToolView::WebSearch { query } | ToolView::Search { pattern: query, .. } => Some(tf("„{}“", &[query])),
        _ => None,
    }
}

fn permission_buttons(ctx: &Ctx<'_>, id: EntryId, request: &PermissionRequest) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let once = ui::button(
        mtm,
        t("Einmal erlauben"),
        ctx.target,
        sel!(ideAgentPermission:),
        pack(id, ALLOW_ONCE),
    );
    if !request.default_to_no {
        once.setKeyEquivalent(&ns(""));
        once.setBezelColor(Some(&NSColor::controlAccentColor()));
    }
    let mut buttons: Vec<Retained<NSView>> = vec![ui::view(&once)];
    let rules: Vec<&PermissionOption> = request
        .options
        .iter()
        .filter(|o| matches!(o, PermissionOption::AllowAlways { .. }))
        .collect();
    if !rules.is_empty() && destructive(request).is_none() {
        let title = if let [PermissionOption::AllowAlways { rule, .. }] = rules.as_slice() {
            tf("Immer erlauben: {}", &[&format::first_line(rule, RULE_LENGTH)])
        } else {
            t("Immer erlauben …").to_string()
        };
        let always = ui::button(
            mtm,
            &title,
            ctx.target,
            sel!(ideAgentPermission:),
            pack(id, ALLOW_ALWAYS),
        );
        if let [PermissionOption::AllowAlways { rule, scope, .. }] = rules.as_slice() {
            let tip = format!("{rule}\n{}", tf("Gilt für {}", &[&scope_name(scope)]));
            always.setToolTip(Some(&ns(&tip)));
        }
        buttons.push(ui::view(&always));
    }
    buttons.push(ui::view(&ui::button(
        mtm,
        t("Ablehnen"),
        ctx.target,
        sel!(ideAgentPermission:),
        pack(id, DENY),
    )));
    buttons.push(ui::spacer(mtm));
    let refs: Vec<&NSView> = buttons.iter().map(|v| &**v).collect();
    ui::view(&ui::row(mtm, 8.0, &refs))
}

/// The input the tool would run with, editable; the same editor as before when the card is built again.
fn editor(ctx: &Ctx<'_>, id: EntryId, request: &PermissionRequest, width: f64) -> Retained<NSScrollView> {
    if let Some(kept) = ctx.inputs.borrow().editors.get(&id.get()) {
        return kept.clone();
    }
    let text = editable_input(&request.tool_name, &request.input);
    let lines = text.lines().count().clamp(1, 10);
    let height = lines as f64 * 15.5 + 10.0;
    let mtm = ctx.mtm;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(lines >= 10);
    scroll.setBorderType(NSBorderType::BezelBorder);
    scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    scroll.heightAnchor().constraintEqualToConstant(height).setActive(true);
    let view = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    view.setRichText(false);
    view.setFont(Some(&style::mono(style::chat(style::CODE))));
    view.setAutomaticQuoteSubstitutionEnabled(false);
    view.setAutomaticDashSubstitutionEnabled(false);
    view.setAutomaticTextReplacementEnabled(false);
    view.setAutomaticSpellingCorrectionEnabled(false);
    view.setTextContainerInset(NSSize::new(4.0, 4.0));
    view.setUserInterfaceLayoutDirection(objc2_app_kit::NSUserInterfaceLayoutDirection::LeftToRight);
    view.setString(&ns(&text));
    view.setAllowsUndo(true);
    view.setAccessibilityLabel(Some(&ns(t("Eingabe des Werkzeugs, vor dem Erlauben änderbar"))));
    scroll.setDocumentView(Some(&view));
    ctx.inputs.borrow_mut().editors.insert(id.get(), scroll.clone());
    scroll
}

/// A text field kept across rebuilds of the card, for free text.
fn field(ctx: &Ctx<'_>, id: EntryId, what: u8, placeholder: &str) -> Retained<NSTextField> {
    if let Some(kept) = ctx.inputs.borrow().fields.get(&(id.get(), what)) {
        return kept.clone();
    }
    let field = NSTextField::textFieldWithString(&ns(""), ctx.mtm);
    field.setPlaceholderString(Some(&ns(placeholder)));
    field.setFont(Some(&style::chat_body()));
    field.setTranslatesAutoresizingMaskIntoConstraints(false);
    ctx.inputs.borrow_mut().fields.insert((id.get(), what), field.clone());
    field
}

pub(crate) fn question(ctx: &Ctx<'_>, id: EntryId, question: &Question) -> Retained<NSView> {
    let mtm = ctx.mtm;
    match &question.state {
        RequestState::Decided(Decision::Answered(answers)) => {
            let lines: Vec<String> = answers
                .iter()
                .map(|a| format!("{} → {}", a.question, a.answer))
                .collect();
            let text = ui::wrapping(
                mtm,
                &lines.join("\n"),
                &style::chat_body(),
                &NSColor::labelColor(),
                ctx.width - 20.0,
            );
            let icon = ui::icon(
                mtm,
                "questionmark.bubble",
                t("Rückfrage"),
                style::chat(12.0),
                &NSColor::secondaryLabelColor(),
            );
            let line = ui::row(mtm, 6.0, &[&icon, &text]);
            line.setAlignment(objc2_app_kit::NSLayoutAttribute::FirstBaseline);
            return ui::view(&line);
        }
        RequestState::Decided(_) | RequestState::Withdrawn => {
            return status_line(
                ctx,
                "questionmark.bubble",
                &NSColor::tertiaryLabelColor(),
                t("Rückfrage ohne Antwort"),
            );
        }
        RequestState::Pending => {}
    }
    let waiting = question.request_id.is_some();
    let mut parts: Vec<Retained<NSView>> = Vec::new();
    for (n, item) in question.items.iter().enumerate().take(16) {
        let index = u8::try_from(n).unwrap_or(0);
        parts.extend(question_item(ctx, id, index, item, waiting));
    }
    let refs: Vec<&NSView> = parts.iter().map(|v| &**v).collect();
    let column = ui::column(mtm, 7.0, &refs);
    ui::view(&ui::framed(
        mtm,
        &column,
        &NSColor::controlAccentColor(),
        None,
        8.0,
        ui::insets(10.0, 12.0, 10.0, 12.0),
    ))
}

fn question_item(ctx: &Ctx<'_>, id: EntryId, index: u8, item: &QuestionItem, waiting: bool) -> Vec<Retained<NSView>> {
    let mtm = ctx.mtm;
    let inner = ctx.width - 26.0;
    let mut parts: Vec<Retained<NSView>> = Vec::new();
    if let Some(header) = &item.header {
        parts.push(ui::view(&ui::label(
            mtm,
            &header.to_uppercase(),
            &style::medium(style::chat(style::SMALL - 1.0)),
            &NSColor::secondaryLabelColor(),
        )));
    }
    parts.push(ui::view(&ui::wrapping(
        mtm,
        &item.question,
        &style::bold(style::chat(style::BODY)),
        &NSColor::labelColor(),
        inner,
    )));
    let chosen = ctx.inputs.borrow().chosen.get(&(id.get(), index)).cloned();
    let mut options: Vec<Retained<NSView>> = Vec::new();
    let mut checks = Vec::new();
    for (o, option) in item.options.iter().enumerate().take(16) {
        let tag = pack(id, index << 4 | u8::try_from(o).unwrap_or(0));
        let button = if item.multi_select {
            // SAFETY: Without a target and an action the checkbox only keeps its state, which `answer_other` reads.
            let check = unsafe { NSButton::checkboxWithTitle_target_action(&ns(&option.label), None, None, mtm) };
            checks.push(check.clone());
            check
        } else {
            let button = ui::button(mtm, &option.label, ctx.target, sel!(ideAgentAnswer:), tag);
            if chosen.as_deref() == Some(option.label.as_str()) {
                button.setButtonType(NSButtonType::PushOnPushOff);
                button.setState(objc2_app_kit::NSControlStateValueOn);
            }
            button
        };
        if let Some(description) = &option.description {
            button.setToolTip(Some(&ns(description)));
        }
        button.setEnabled(waiting);
        options.push(ui::view(&button));
    }
    if item.multi_select {
        ctx.inputs.borrow_mut().checks.insert((id.get(), index), checks);
    }
    options.push(ui::spacer(mtm));
    let refs: Vec<&NSView> = options.iter().map(|v| &**v).collect();
    let row = ui::row(mtm, 6.0, &refs);
    if item.multi_select {
        row.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        row.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    }
    parts.push(ui::view(&row));
    let described: Vec<String> = item
        .options
        .iter()
        .filter_map(|o| o.description.as_ref().map(|d| format!("{} — {d}", o.label)))
        .collect();
    if !described.is_empty() {
        parts.push(ui::view(&ui::wrapping(
            mtm,
            &described.join("\n"),
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
            inner,
        )));
    }
    let own = field(ctx, id, index, t("Andere Antwort …"));
    own.setEnabled(waiting);
    let send = ui::button(
        mtm,
        t("Senden"),
        ctx.target,
        sel!(ideAgentAnswerOther:),
        pack(id, index),
    );
    send.setEnabled(waiting);
    parts.push(ui::view(&ui::row(mtm, 6.0, &[&own, &send])));
    parts
}

pub(crate) fn plan(ctx: &Ctx<'_>, id: EntryId, plan: &PlanApproval) -> Retained<NSView> {
    if plan.state == RequestState::Pending {
        pending_plan(ctx, id, plan)
    } else {
        decided_plan(ctx, id, plan)
    }
}

/// What became of an answered plan: its symbol, colour and line, and the comments it went back with.
pub(crate) fn plan_outcome(plan: &PlanApproval) -> (&'static str, Retained<NSColor>, &'static str, Option<&str>) {
    let returned = "arrow.uturn.backward.circle";
    match &plan.state {
        RequestState::Decided(Decision::Approved) => (
            "checkmark.circle",
            NSColor::systemGreenColor(),
            t("Plan angenommen"),
            None,
        ),
        RequestState::Decided(Decision::Rejected { stop: true, .. }) => {
            ("xmark.circle", NSColor::systemRedColor(), t("Plan abgelehnt"), None)
        }
        RequestState::Decided(Decision::Rejected {
            feedback: Some(comments),
            ..
        }) => (
            returned,
            NSColor::systemOrangeColor(),
            t("Plan mit Kommentaren zurückgegeben"),
            Some(comments.as_str()),
        ),
        RequestState::Decided(Decision::Rejected { .. }) => {
            (returned, NSColor::systemOrangeColor(), t("Plan zurückgegeben"), None)
        }
        _ => (
            "list.bullet.clipboard",
            NSColor::tertiaryLabelColor(),
            t("Plan ohne Entscheidung"),
            None,
        ),
    }
}

/// A plan that was answered: what became of it, the comments it went back with, and the plan when opened.
fn decided_plan(ctx: &Ctx<'_>, id: EntryId, plan: &PlanApproval) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let (symbol, color, text, comments) = plan_outcome(plan);
    let icon = ui::icon(mtm, symbol, text, style::chat(11.0), &color);
    let label = ui::label(mtm, text, &style::chat_small(), &NSColor::secondaryLabelColor());
    let chevron = disclosure(ctx, id, Part::Plan);
    let spacer = ui::spacer(mtm);
    let line = ui::row(mtm, 5.0, &[&icon, &label, &chevron, &spacer]);
    let mut parts: Vec<Retained<NSView>> = vec![ui::view(&line)];
    if let Some(comments) = comments {
        let shown = markdown_text(&ctx.narrower(22.0), comments, Tone::Secondary);
        let yellow = NSColor::systemYellowColor();
        parts.push(ui::view(&ui::framed(
            mtm,
            &shown,
            &yellow.colorWithAlphaComponent(0.55),
            Some(&yellow.colorWithAlphaComponent(0.10)),
            6.0,
            ui::insets(6.0, 10.0, 6.0, 10.0),
        )));
    }
    if ctx.is_open(id, Part::Plan) {
        parts.push(ui::view(&markdown_text(ctx, &plan.plan, Tone::Secondary)));
    }
    let parts: Vec<&NSView> = parts.iter().map(|p| &**p).collect();
    ui::view(&ui::column(mtm, 6.0, &parts))
}

/// A plan that waits: the plan, a comment for the agent, and the three answers.
fn pending_plan(ctx: &Ctx<'_>, id: EntryId, plan: &PlanApproval) -> Retained<NSView> {
    let mtm = ctx.mtm;
    let inner = ctx.narrower(26.0);
    let icon = ui::icon(
        mtm,
        "list.bullet.clipboard",
        t("Plan"),
        style::chat(13.0),
        &NSColor::secondaryLabelColor(),
    );
    let title = ui::label(
        mtm,
        t("Plan zur Freigabe"),
        &style::bold(style::chat(style::BODY)),
        &NSColor::labelColor(),
    );
    let spacer = ui::spacer(mtm);
    let head = ui::row(mtm, 10.0, &[&icon, &title, &spacer]);
    let rendered = super::super::markdown::render(mtm, &plan.plan, style::chat(style::BODY), Tone::Primary);
    let body = CardText::new(mtm, &rendered, inner.width, true);
    ctx.inputs
        .borrow_mut()
        .plan_text
        .insert(id.get(), Retained::into_super(body.clone()));
    let waiting = plan.request_id.is_some();
    let general = field(ctx, id, 0, t("Kommentar für Claude (optional)"));
    general.setEnabled(waiting);
    let answers = plan_answers(ctx, id, waiting);
    let column = ui::column(mtm, 8.0, &[&head, &body, &general, &answers]);
    ui::view(&ui::framed(
        mtm,
        &column,
        &NSColor::controlAccentColor(),
        None,
        8.0,
        ui::insets(10.0, 12.0, 10.0, 12.0),
    ))
}

/// The answers to a plan.
fn plan_answers(ctx: &Ctx<'_>, id: EntryId, waiting: bool) -> Retained<NSStackView> {
    let mtm = ctx.mtm;
    let approve = ui::button(
        mtm,
        t("Plan annehmen"),
        ctx.target,
        sel!(ideAgentPlan:),
        pack(id, PLAN_APPROVE),
    );
    approve.setBezelColor(Some(&NSColor::controlAccentColor()));
    let revise = ui::button(
        mtm,
        t("Überarbeiten lassen"),
        ctx.target,
        sel!(ideAgentPlan:),
        pack(id, PLAN_REVISE),
    );
    let reject = ui::button(
        mtm,
        t("Plan ablehnen"),
        ctx.target,
        sel!(ideAgentPlan:),
        pack(id, PLAN_REJECT),
    );
    reject.setToolTip(Some(&ns(t("Der Plan wird nicht umgesetzt, der Agent hält an"))));
    for button in [&approve, &revise, &reject] {
        button.setEnabled(waiting);
    }
    let spacer = ui::spacer(mtm);
    ui::row(mtm, 8.0, &[&approve, &revise, &reject, &spacer])
}

/// Why a git command in a request cannot be undone, in the user's words.
fn destructive_text(why: term_thread::shell::Destructive) -> String {
    use term_thread::shell::Destructive;
    match why {
        Destructive::ForcePush => t("git push --force überschreibt den Verlauf auf dem Server"),
        Destructive::ResetHard => t("git reset --hard verwirft nicht gesicherte Änderungen"),
        Destructive::Clean => t("git clean -f löscht Dateien, die git nicht verfolgt"),
        Destructive::StashDrop => t("git stash drop löscht einen Stash endgültig"),
        Destructive::StashClear => t("git stash clear löscht alle Stashes endgültig"),
    }
    .to_string()
}

/// The labels checked in a question that takes several.
pub(crate) fn checked(buttons: &[Retained<NSButton>]) -> Vec<String> {
    buttons
        .iter()
        .filter(|b| b.state() == objc2_app_kit::NSControlStateValueOn)
        .map(|b| b.title().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_command_is_edited_as_such_and_other_input_as_json() {
        let input = json!({"command": "ls", "description": "list"});
        assert_eq!(editable_input("Bash", &input), "ls");
        let edited = edited_input("Bash", &input, "ls -la").unwrap();
        assert_eq!(edited, json!({"command": "ls -la", "description": "list"}));
        let write = json!({"file_path": "a", "content": "x"});
        assert_eq!(editable_input("Write", &write), "x");
        assert_eq!(
            edited_input("Write", &write, "y"),
            Some(json!({"file_path": "a", "content": "y"}))
        );
        let change = json!({"file_path": "a", "old_string": "world", "new_string": "there"});
        assert_eq!(editable_input("Edit", &change), "there");
        assert_eq!(edited_input("Edit", &change, "you").unwrap()["new_string"], "you");
        let many = json!({"file_path": "a", "edits": []});
        assert_eq!(
            edited_input("MultiEdit", &many, &editable_input("MultiEdit", &many)),
            Some(many.clone())
        );
        assert_eq!(edited_input("MultiEdit", &many, "not json"), None);
    }

    #[test]
    fn destructive_git_commands_never_get_a_stored_rule() {
        let request = |tool: &str, title: Option<&str>, input: Value| PermissionRequest {
            request_id: "r".into(),
            tool_call: None,
            tool_name: tool.into(),
            title: title.map(str::to_string),
            description: None,
            reason: None,
            blocked_path: None,
            input,
            options: Vec::new(),
            default_to_no: false,
            state: RequestState::Pending,
        };
        let bash = |c: &str| request("Bash", None, json!({ "command": c }));
        assert!(destructive(&bash("git push --force origin main")).is_some());
        assert!(destructive(&bash("cd x && git reset --hard HEAD~1")).is_some());
        assert!(destructive(&bash("git clean -fd")).is_some());
        assert!(destructive(&bash("sh -c 'git push -f'")).is_some());
        assert!(destructive(&bash("git status")).is_none());
        assert!(destructive(&bash("git push origin main")).is_none());
        let acp = request("execute", Some("git -C repo reset --hard"), Value::Null);
        assert!(destructive(&acp).is_some());
        let write = request(
            "Write",
            Some("Write notes.md"),
            json!({"file_path": "notes.md", "content": "git clean -fd"}),
        );
        assert!(destructive(&write).is_none());
    }
}
