//! One card per timeline entry, built only when its row comes into view.

mod notice;

#[cfg(feature = "selftest")]
pub use notice::text as notice_text;
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub use notice::window_name;
pub(crate) mod request;
mod subagent;
pub(crate) use subagent::status_text;
mod tool;
mod turn;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSButton, NSColor, NSFont, NSScrollView, NSStackView, NSTextField, NSTextView, NSView};
use splat_i18n::{t, tf};
use term_thread::{
    Decision, Entry, EntryId, EntryKind, ErrorKind, HookOutcome, InfoLevel, LimitStatus, Notice, RequestState, Thread,
    TodoStatus, ToolOutput, ToolStatus, ToolView,
};

use super::format;
use super::markdown::{self, Tone};
use super::row::Mark;
use super::style;
use super::text::CardText;
use super::ui;

/// The parts of a card that open and close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub enum Part {
    /// A thought's text.
    Thought = 1,
    /// A tool call's input.
    Input = 2,
    /// A tool call's output.
    Output = 3,
    /// What a subagent did.
    Subagent = 4,
    /// A plan to approve.
    Plan = 5,
    /// A notice's details.
    Detail = 6,
}

impl Part {
    pub(crate) fn from_tag(tag: u8) -> Option<Part> {
        [
            Part::Thought,
            Part::Input,
            Part::Output,
            Part::Subagent,
            Part::Plan,
            Part::Detail,
        ]
        .into_iter()
        .find(|p| *p as u8 == tag)
    }
}

/// The tag of a control that acts on entry `id`, with a number for what it does.
pub(crate) fn pack(id: EntryId, what: u8) -> isize {
    isize::try_from(id.get() << 8 | u64::from(what)).unwrap_or(-1)
}

/// The entry number and the extra number of a control's tag.
pub(crate) fn unpack(tag: isize) -> Option<(u64, u8)> {
    let tag = u64::try_from(tag).ok()?;
    Some((tag >> 8, (tag & 0xff) as u8))
}

/// What the user typed into cards that wait for an answer; kept when a card is built again.
#[derive(Default)]
pub struct Inputs {
    /// The editable input of a permission request, in its scroll view.
    pub(crate) editors: HashMap<u64, Retained<NSScrollView>>,
    /// Free text: a question's own answer, a plan's comment.
    pub(crate) fields: HashMap<(u64, u8), Retained<NSTextField>>,
    /// The options checked in a question with several answers.
    pub(crate) checks: HashMap<(u64, u8), Vec<Retained<NSButton>>>,
    /// The option chosen for each question of a request that asks several.
    pub(crate) chosen: HashMap<(u64, u8), String>,
    /// The text of a waiting plan's card, where the user marks the passage to comment on.
    pub plan_text: HashMap<u64, Retained<NSTextView>>,
    /// Pictures the decoding service turned into pixels, by their bytes, so a card built again shows them at once.
    pub(crate) pictures: ui::Pictures,
}

impl Inputs {
    /// The text of the editor of request `id`.
    pub(crate) fn editor_text(&self, id: u64) -> Option<String> {
        let scroll = self.editors.get(&id)?;
        let view = scroll.documentView()?.downcast::<NSTextView>().ok()?;
        Some(view.string().to_string())
    }

    /// The text typed into field `what` of entry `id`.
    pub(crate) fn field_text(&self, id: u64, what: u8) -> Option<String> {
        self.fields
            .get(&(id, what))
            .map(|f| f.stringValue().to_string())
            .filter(|s| !s.trim().is_empty())
    }

    /// Forgets what belongs to entries that no longer wait.
    pub(crate) fn forget(&mut self, id: u64) {
        self.editors.remove(&id);
        self.fields.retain(|(entry, _), _| *entry != id);
        self.checks.retain(|(entry, _), _| *entry != id);
        self.chosen.retain(|(entry, _), _| *entry != id);
        self.plan_text.remove(&id);
    }

    /// Forgets the input editors of the permission requests that `changes` answered or withdrew in `thread`, however
    /// they were answered.
    pub(crate) fn forget_answered(&mut self, thread: &Thread, changes: &[term_thread::Change]) {
        if self.editors.is_empty() {
            return;
        }
        for change in changes {
            if let term_thread::Change::Updated { id, .. } = change
                && thread.entry(*id).is_some_and(
                    |entry| matches!(&entry.kind, EntryKind::PermissionRequest(r) if r.state != RequestState::Pending),
                )
            {
                self.editors.remove(&id.get());
            }
        }
    }
}

/// A label whose running time the tab advances every second.
pub(crate) struct Live {
    pub(crate) label: Retained<NSTextField>,
    pub(crate) since: SystemTime,
}

/// What a card is built with.
#[derive(Clone, Copy)]
pub(crate) struct Ctx<'a> {
    pub(crate) mtm: MainThreadMarker,
    /// The width the card has.
    pub(crate) width: f64,
    pub(crate) thread: &'a Thread,
    pub(crate) expanded: &'a HashSet<(u64, Part)>,
    pub(crate) target: &'a AnyObject,
    pub(crate) cwd: &'a Path,
    pub(crate) inputs: &'a RefCell<Inputs>,
    pub(crate) live: &'a RefCell<Vec<Live>>,
    /// Inside a subagent's card, where cards are smaller.
    pub(crate) nested: bool,
    pub(crate) now: SystemTime,
}

impl Ctx<'_> {
    pub(crate) fn is_open(&self, id: EntryId, part: Part) -> bool {
        self.expanded.contains(&(id.get(), part))
    }

    /// The same context for a card `inset` points narrower.
    pub(crate) fn narrower(&self, inset: f64) -> Ctx<'_> {
        Ctx {
            width: (self.width - inset).max(60.0),
            ..*self
        }
    }

    /// A label counting up from `since`.
    pub(crate) fn running_label(&self, since: SystemTime) -> Retained<NSTextField> {
        let label = ui::label(
            self.mtm,
            &format::elapsed(self.now.duration_since(since).unwrap_or_default()),
            &style::chat_small(),
            &NSColor::secondaryLabelColor(),
        );
        ui::hug(&label);
        self.live.borrow_mut().push(Live {
            label: label.clone(),
            since,
        });
        label
    }
}

/// The card of `entry`.
pub(crate) fn build(ctx: &Ctx<'_>, entry: &Entry) -> Retained<NSView> {
    let view = match &entry.kind {
        EntryKind::UserMessage(message) => turn::user(ctx, entry, message),
        EntryKind::AgentMessage(message) => turn::answer(ctx, entry.id, &message.markdown, message.streaming),
        EntryKind::Thought(thought) => turn::thought(ctx, entry.id, thought),
        EntryKind::ToolCall(call) => tool::card(ctx, entry.id, call),
        EntryKind::Todos(todos) => turn::todos(ctx, todos),
        EntryKind::PermissionRequest(request) => request::permission(ctx, entry.id, request),
        EntryKind::Question(question) => request::question(ctx, entry.id, question),
        EntryKind::PlanApproval(plan) => request::plan(ctx, entry.id, plan),
        EntryKind::Notice(Notice::Hook {
            outcome: HookOutcome::Success,
            ..
        }) if !ctx.nested => notice::hook_group(ctx, entry),
        EntryKind::Notice(notice) => notice::card(ctx, entry.id, notice),
    };
    ui::group(&view, &summary(ctx.thread, entry));
    view
}

/// Markdown in a text view as wide as the context.
pub(crate) fn markdown_text(ctx: &Ctx<'_>, markdown: &str, tone: Tone) -> Retained<CardText> {
    let text = markdown::render_with_figures(ctx.mtm, markdown, style::chat(style::BODY), tone, ctx.width);
    CardText::new(ctx.mtm, &text, ctx.width, true)
}

/// Monospaced text in a text view `width` points wide, left to right in any interface.
pub(crate) fn mono_text(ctx: &Ctx<'_>, text: &str, width: f64, color: &NSColor) -> Retained<CardText> {
    let shown = markdown::code(text, &style::mono(style::chat(style::CODE)), color);
    let view = CardText::new(ctx.mtm, &shown, width, true);
    view.left_to_right();
    view
}

/// A chevron that opens or closes `part` of entry `id`.
pub(crate) fn disclosure(ctx: &Ctx<'_>, id: EntryId, part: Part) -> Retained<NSButton> {
    let open = ctx.is_open(id, part);
    let (name, description) = if open {
        ("chevron.down", t("Einklappen"))
    } else {
        ("chevron.forward", t("Aufklappen"))
    };
    let button = ui::symbol_button(
        ctx.mtm,
        name,
        description,
        style::chat(9.0),
        ctx.target,
        sel!(ideAgentToggle:),
        pack(id, part as u8),
    );
    ui::hug(&button);
    button
}

/// The head line of a card: a bold name, a quieter detail, then whatever goes on the trailing side.
pub(crate) fn head(ctx: &Ctx<'_>, name: &str, detail: &str, trailing: &[&NSView]) -> Retained<NSStackView> {
    let size = style::chat(if ctx.nested { style::BODY - 1.0 } else { style::BODY });
    let name = ui::label(ctx.mtm, name, &style::bold(size), &NSColor::labelColor());
    ui::hug(&name);
    let detail = ui::label(
        ctx.mtm,
        &head_detail(detail),
        &NSFont::systemFontOfSize(size),
        &NSColor::secondaryLabelColor(),
    );
    let spacer = ui::spacer(ctx.mtm);
    let mut views: Vec<&NSView> = vec![&name, &detail, &spacer];
    views.extend(trailing.iter().copied());
    ui::row(ctx.mtm, 6.0, &views)
}

/// The most characters of a head's detail; the label cuts it shorter where the card is narrower.
const HEAD_DETAIL: usize = 200;

/// The detail of a card's head on one line, such as a shell command without a description, which the card shows in
/// full below.
fn head_detail(detail: &str) -> String {
    format::first_line(detail, HEAD_DETAIL)
}

/// The colour of the dot of `entry` in the timeline; `None` for entries without one.
fn dot(entry: &Entry) -> Option<Retained<NSColor>> {
    Some(match &entry.kind {
        EntryKind::UserMessage(_) => return None,
        EntryKind::AgentMessage(_) => NSColor::labelColor(),
        EntryKind::Thought(_) | EntryKind::Todos(_) => NSColor::tertiaryLabelColor(),
        EntryKind::ToolCall(call) => match call.status {
            ToolStatus::Completed => NSColor::systemGreenColor(),
            ToolStatus::Failed => NSColor::systemRedColor(),
            ToolStatus::InProgress => NSColor::systemOrangeColor(),
            ToolStatus::Pending => NSColor::tertiaryLabelColor(),
        },
        EntryKind::PermissionRequest(r) => match r.state {
            RequestState::Pending => NSColor::systemOrangeColor(),
            RequestState::Decided(Decision::Denied { .. }) => NSColor::systemRedColor(),
            _ => NSColor::tertiaryLabelColor(),
        },
        EntryKind::Question(q) if q.state == RequestState::Pending => NSColor::controlAccentColor(),
        EntryKind::PlanApproval(p) if p.state == RequestState::Pending => NSColor::controlAccentColor(),
        EntryKind::Question(_) | EntryKind::PlanApproval(_) => NSColor::tertiaryLabelColor(),
        EntryKind::Notice(Notice::Error { .. }) => NSColor::systemRedColor(),
        EntryKind::Notice(Notice::Retrying { .. } | Notice::RateLimit(_)) => NSColor::systemOrangeColor(),
        EntryKind::Notice(_) => NSColor::tertiaryLabelColor(),
    })
}

/// How the row at `index` of `entries` is marked: its dot, and whether the line joins it to its neighbours. A user
/// message stands apart and breaks the line.
pub(crate) fn mark(entries: &[std::sync::Arc<Entry>], index: usize) -> Mark {
    let is_user = |i: usize| {
        entries
            .get(i)
            .is_some_and(|e| matches!(e.kind, EntryKind::UserMessage(_)))
    };
    let Some(entry) = entries.get(index) else {
        return Mark::default();
    };
    if is_user(index) {
        return Mark::default();
    }
    Mark {
        dot: dot(entry),
        line_above: index > 0 && !is_user(index - 1),
        line_below: index + 1 < entries.len() && !is_user(index + 1),
    }
}

/// A guess at a card's height before it is built, so the timeline's length is right without building every card.
pub(crate) fn estimate(entry: &Entry, width: f64) -> f64 {
    let scale = style::chat(1.0);
    scale_estimate(entry, width / scale) * scale
}

/// `estimate` at the default text size.
fn scale_estimate(entry: &Entry, width: f64) -> f64 {
    let per_line = (width / 7.0).max(20.0);
    let lines = |text: &str| -> f64 {
        text.lines()
            .map(|l| (l.chars().count() as f64 / per_line).ceil().max(1.0))
            .sum::<f64>()
            .max(1.0)
    };
    match &entry.kind {
        EntryKind::UserMessage(m) => 44.0 + lines(&m.text) * 17.0,
        EntryKind::AgentMessage(m) => 8.0 + lines(&m.markdown) * 19.0,
        EntryKind::Thought(_) => 26.0,
        EntryKind::ToolCall(call) => match call.view {
            ToolView::Agent { .. } => 86.0,
            ToolView::Read { .. } | ToolView::Search { .. } => 30.0,
            _ => 96.0,
        },
        EntryKind::Todos(todos) => 30.0 + todos.len() as f64 * 20.0,
        EntryKind::PermissionRequest(r) if r.state == RequestState::Pending => 150.0,
        EntryKind::Question(q) if q.state == RequestState::Pending => 120.0,
        EntryKind::PlanApproval(p) if p.state == RequestState::Pending => 100.0 + lines(&p.plan) * 19.0,
        _ => 28.0,
    }
}

/// What VoiceOver reads for a card.
pub(crate) fn summary(thread: &Thread, entry: &Entry) -> String {
    match &entry.kind {
        EntryKind::UserMessage(m) => tf("Deine Nachricht: {}", &[&format::first_line(&m.text, 200)]),
        EntryKind::AgentMessage(m) => tf("Antwort: {}", &[&format::first_line(&m.markdown, 200)]),
        EntryKind::Thought(_) => t("Denken").to_string(),
        EntryKind::ToolCall(call) => {
            let state = match call.status {
                ToolStatus::Pending => t("wartet"),
                ToolStatus::InProgress => t("läuft"),
                ToolStatus::Completed => t("fertig"),
                ToolStatus::Failed => t("fehlgeschlagen"),
            };
            let detail = if let Some(sub) = thread.subagent(&call.call_id) {
                sub.description.clone()
            } else {
                call.title.clone()
            };
            tf("Werkzeug {}: {}, {}", &[&call.name, &detail, state])
        }
        EntryKind::Todos(todos) => {
            let done = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
            tf(
                "To-dos: {} von {} erledigt",
                &[&done.to_string(), &todos.len().to_string()],
            )
        }
        EntryKind::PermissionRequest(r) => tf("Freigabe für {}", &[&r.tool_name]),
        EntryKind::Question(q) => tf(
            "Rückfrage: {}",
            &[&q.items.first().map(|i| i.question.clone()).unwrap_or_default()],
        ),
        EntryKind::PlanApproval(p) if p.state == RequestState::Pending => t("Plan zur Freigabe").to_string(),
        EntryKind::PlanApproval(p) => request::plan_outcome(p).2.to_string(),
        EntryKind::Notice(Notice::Hook {
            outcome: HookOutcome::Success,
            ..
        }) => notice::hook_group_text(thread, entry),
        EntryKind::Notice(n) => notice::text(n),
    }
}

/// The colour a notice is written in.
pub(crate) fn notice_color(notice: &Notice) -> Retained<NSColor> {
    match notice {
        Notice::Error { .. } => NSColor::systemRedColor(),
        Notice::Retrying { .. } | Notice::RewindRejected { .. } => NSColor::systemOrangeColor(),
        Notice::RateLimit(limit) if limit.status == LimitStatus::Rejected => NSColor::systemRedColor(),
        Notice::RateLimit(_) => NSColor::systemOrangeColor(),
        Notice::Info {
            level: InfoLevel::Warning,
            ..
        } => NSColor::systemOrangeColor(),
        _ => NSColor::secondaryLabelColor(),
    }
}

/// Whether the error notice asks the user to sign in.
pub(crate) fn is_auth(notice: &Notice) -> bool {
    matches!(
        notice,
        Notice::Error {
            kind: ErrorKind::Auth,
            ..
        }
    )
}

/// The lines of a tool's output, as text.
pub(crate) fn output_text(output: &ToolOutput) -> String {
    match output {
        ToolOutput::Text(text) => text.clone(),
        ToolOutput::Terminal { stdout, stderr, .. } => match (stdout.trim_end(), stderr.trim_end()) {
            (out, "") => out.to_string(),
            ("", err) => err.to_string(),
            (out, err) => format!("{out}\n{err}"),
        },
        ToolOutput::Diff { hunks, .. } => hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// How long ago `since` was, as a duration.
pub(crate) fn since(ctx: &Ctx<'_>, since: SystemTime) -> Duration {
    ctx.now.duration_since(since).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heads_detail_keeps_to_its_first_line() {
        assert_eq!(head_detail("Run the tests"), "Run the tests");
        assert_eq!(head_detail("python3 - <<'EOF'\np='a'\nEOF"), "python3 - <<'EOF'…");
        assert_eq!(head_detail("\n  cargo test\n"), "cargo test");
    }

    #[test]
    fn a_tag_carries_the_entry_and_what_to_do() {
        let tag = pack(thread_entry(42), Part::Output as u8);
        assert_eq!(unpack(tag), Some((42, Part::Output as u8)));
        assert_eq!(Part::from_tag(Part::Output as u8), Some(Part::Output));
    }

    fn thread_entry(n: u64) -> EntryId {
        let mut thread = Thread::new(term_thread::ThreadId::from("t"), "claude", "/w");
        let mut id = None;
        for i in 0..n {
            let changes = thread.apply(term_thread::Update::Put {
                lane: term_thread::Lane::Main,
                key: Some(format!("k{i}")),
                at: SystemTime::now(),
                entry: EntryKind::Thought(term_thread::Thought::default()),
            });
            if let Some(term_thread::Change::Inserted { id: new, .. }) = changes.first() {
                id = Some(*new);
            }
        }
        id.unwrap()
    }
}
