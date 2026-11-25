//! What fills an agent tab's context window, row by row as Claude's own `/context` view breaks it down, in a popover
//! that opens from the status line's gauge; and the cost texts of the status line and the session list. The
//! breakdown is experimental with Claude (E21): while the CLI has given none, the gauge opens nothing.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::time::SystemTime;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSBezierPath, NSButton, NSColor, NSLayoutConstraintOrientation, NSPopover, NSPopoverBehavior,
    NSScrollView, NSStackView, NSView, NSViewController,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSRectEdge, NSSize};
use splat_i18n::{t, tf};
use term_thread::{ContextBreakdown, ContextGroup, ContextKind, ContextPart, ContextRow, Usage};

use super::format::{dollars, elapsed, percent, tokens};
use super::{style, ui};
use crate::AgentKey;
use crate::common::util::{activate, guarded, ns, pin_inset, set_action};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

/// The width of the popover's content.
const WIDTH: f64 = 340.0;
/// The tallest the popover grows before its content scrolls.
const MAX_HEIGHT: f64 = 460.0;
/// Parts listed per group before the rest is counted.
const PARTS_SHOWN: usize = 6;

thread_local! {
    static OPENER: OnceCell<Retained<Opener>> = const { OnceCell::new() };
}

/// The context gauges of the agent tabs and the breakdown on view.
#[derive(Default)]
pub(crate) struct ContextGauges {
    /// The gauge of each tab, by the tab's key.
    gauges: RefCell<HashMap<AgentKey, Weak<NSButton>>>,
    shown: RefCell<Option<Shown>>,
}

fn with_gauges<R>(f: impl FnOnce(&RefCell<HashMap<AgentKey, Weak<NSButton>>>) -> R) -> R {
    f(&crate::host::state().context_gauges.gauges)
}

fn with_shown<R>(f: impl FnOnce(&RefCell<Option<Shown>>) -> R) -> R {
    f(&crate::host::state().context_gauges.shown)
}

/// The popover on show, the tab it belongs to and the breakdown it shows.
struct Shown {
    key: AgentKey,
    popover: Retained<NSPopover>,
    controller: Retained<NSViewController>,
    /// When the breakdown on show was answered, and whether it was counted.
    answered: (SystemTime, bool),
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Opener` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentContextOpener"]
    pub(crate) struct Opener;

    unsafe impl NSObjectProtocol for Opener {}

    impl Opener {
        #[unsafe(method(ideAgentContext:))]
        fn open(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(button) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                    show(key_of(button), button);
                }
            });
        }
    }
);

fn opener(mtm: MainThreadMarker) -> Retained<Opener> {
    OPENER.with(|cell| {
        cell.get_or_init(|| {
            // SAFETY: NSObject's initializer, called once on the newly allocated object.
            unsafe { msg_send![Opener::alloc(mtm), init] }
        })
        .clone()
    })
}

/// A button without a look of its own over the gauge, from `ring` to `text` in the status line `row`, that opens the
/// breakdown of the tab with `key`; hidden until there is one.
pub(crate) fn gauge(
    mtm: MainThreadMarker,
    key: AgentKey,
    row: &NSStackView,
    ring: &NSView,
    text: &NSView,
) -> Retained<NSButton> {
    // SAFETY: A nil target until `set_action` below, which checks that the opener answers the action.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(""), None, None, mtm) };
    button.setBordered(false);
    button.setTransparent(true);
    button.setTag(isize::try_from(key.get()).unwrap_or(0));
    set_action(&button, &opener(mtm), sel!(ideAgentContext:));
    crate::common::util::set_label(&button, Some(t("Kontext aufschlüsseln")));
    button.setToolTip(Some(&ns(t("Was den Kontext füllt"))));
    button.setHidden(true);
    row.addSubview(&button);
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        button.leadingAnchor().constraintEqualToAnchor(&ring.leadingAnchor()),
        button.trailingAnchor().constraintEqualToAnchor(&text.trailingAnchor()),
        button.topAnchor().constraintEqualToAnchor(&row.topAnchor()),
        button.bottomAnchor().constraintEqualToAnchor(&row.bottomAnchor()),
    ]);
    with_gauges(|gauges| gauges.borrow_mut().insert(key, Weak::from(&*button)));
    button
}

/// The agent tab `gauge` belongs to, from its tag.
fn key_of(gauge: &NSButton) -> AgentKey {
    AgentKey(u64::try_from(gauge.tag()).unwrap_or(0))
}

/// Lets `gauge` open the breakdown once `usage` has one, and shows a new one in the popover if it is open.
pub(crate) fn follow(gauge: &NSButton, usage: &Usage) {
    gauge.setHidden(usage.breakdown.is_none());
    let Some(breakdown) = &usage.breakdown else { return };
    let key = key_of(gauge);
    let answered = (breakdown.at, breakdown.exact);
    let open = with_shown(|shown| {
        let mut shown = shown.borrow_mut();
        let open = shown
            .as_mut()
            .filter(|s| s.key == key && s.popover.isShown() && s.answered != answered)?;
        open.answered = answered;
        Some((open.popover.clone(), open.controller.clone()))
    });
    if let Some((popover, controller)) = open {
        let view = content(gauge.mtm(), breakdown);
        popover.setContentSize(view.fittingSize());
        controller.setView(&view);
    }
}

/// Opens the breakdown of the tab with `key` at `anchor`, and has the CLI count every row exactly.
fn show(key: AgentKey, anchor: &NSView) {
    let Some(tab) = super::get(key) else { return };
    let thread = tab.snapshot();
    let Some(breakdown) = &thread.usage().breakdown else {
        return;
    };
    if let Some(shown) = with_shown(|shown| shown.borrow_mut().take()) {
        shown.popover.close();
    }
    let mtm = anchor.mtm();
    let view = content(mtm, breakdown);
    let controller = NSViewController::new(mtm);
    controller.setView(&view);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setAnimates(false);
    popover.setContentViewController(Some(&controller));
    popover.setContentSize(view.fittingSize());
    popover.showRelativeToRect_ofView_preferredEdge(anchor.bounds(), anchor, NSRectEdge::MaxY);
    with_shown(|shown| {
        *shown.borrow_mut() = Some(Shown {
            key,
            popover,
            controller,
            answered: (breakdown.at, breakdown.exact),
        });
    });
    if !breakdown.exact
        && let Some(session) = tab.session()
    {
        drop(session.context_usage(term_claude::control::ContextCount::Exact));
    }
}

/// The popover's content: the totals, a bar of the window, the rows and what some rows consist of.
fn content(mtm: MainThreadMarker, breakdown: &ContextBreakdown) -> Retained<NSView> {
    let size = breakdown.size.max(1);
    let share = |n: u64| super::format::percent(n as f64 / size as f64);
    let title = ui::label(mtm, t("Kontext"), &style::bold(13.0), &NSColor::labelColor());
    let totals = ui::label(
        mtm,
        &tf(
            "{} von {} Tokens · {}",
            &[&tokens(breakdown.used), &tokens(breakdown.size), &share(breakdown.used)],
        ),
        &style::small(),
        &NSColor::secondaryLabelColor(),
    );
    let how = if breakdown.exact {
        t("gezählt")
    } else {
        t("geschätzt aus der letzten Antwort")
    };
    let model = ui::label(
        mtm,
        &format!("{} · {how}", breakdown.model),
        &style::small(),
        &NSColor::tertiaryLabelColor(),
    );
    let bar = Bar::new(mtm, breakdown);
    let mut views: Vec<Retained<NSView>> = vec![
        ui::view(&title),
        ui::view(&totals),
        ui::view(&model),
        Retained::into_super(bar),
    ];
    for row in &breakdown.rows {
        views.push(row_view(mtm, row, &share(row.tokens)));
    }
    if let Some(at) = breakdown.auto_compact_at {
        views.push(ui::view(&ui::label(
            mtm,
            &tf("Verdichtet sich selbst ab {} Tokens", &[&tokens(at)]),
            &style::small(),
            &NSColor::secondaryLabelColor(),
        )));
    }
    for group in GROUPS {
        let parts: Vec<&ContextPart> = breakdown.parts.iter().filter(|p| p.group == group).collect();
        if !parts.is_empty() {
            views.extend(group_views(mtm, group, &parts));
        }
    }
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    let column = padded(mtm, &ui::column(mtm, 5.0, &refs));
    column.layoutSubtreeIfNeeded();
    let height = column.fittingSize().height;
    if height <= MAX_HEIGHT {
        return column;
    }
    let scroll = NSScrollView::new(mtm);
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let holder: Retained<TopDown> = unsafe { msg_send![TopDown::alloc(mtm), init] };
    holder.addSubview(&column);
    column.setTranslatesAutoresizingMaskIntoConstraints(true);
    column.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)));
    holder.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, height)));
    scroll.setDocumentView(Some(&holder));
    ui::fixed_width(&scroll, WIDTH);
    ui::fixed_height(&scroll, MAX_HEIGHT);
    Retained::into_super(scroll)
}

/// `column` with room around it, `WIDTH` wide; a stack view's own insets do not reach views as wide as the stack.
fn padded(mtm: MainThreadMarker, column: &NSStackView) -> Retained<NSView> {
    let holder = NSView::new(mtm);
    holder.addSubview(column);
    ui::fixed_width(&holder, WIDTH);
    pin_inset(column, &holder, ui::insets(12.0, 14.0, 12.0, 14.0));
    holder
}

/// An amount in dollars, with one too small for the format said to be below its smallest step.
fn amount(cost: f64) -> String {
    if cost > 0.0 && cost < SMALLEST {
        tf("< {}", &[&dollars(SMALLEST)])
    } else {
        dollars(cost)
    }
}

/// The smallest amount `dollars` shows.
const SMALLEST: f64 = 0.001;

/// One row: its colour, its name, its tokens and its share of the window.
fn row_view(mtm: MainThreadMarker, row: &ContextRow, share: &str) -> Retained<NSView> {
    let swatch = Swatch::new(mtm, &color_of(row));
    let name = ui::label(mtm, &row_name(row), &style::small(), &NSColor::labelColor());
    let amount = ui::label(
        mtm,
        &tokens(row.tokens),
        &style::small(),
        &NSColor::secondaryLabelColor(),
    );
    let part = if row.kind == ContextKind::Deferred {
        t("außerhalb").to_string()
    } else {
        share.to_string()
    };
    let percent = ui::label(mtm, &part, &style::small(), &NSColor::tertiaryLabelColor());
    for label in [&amount, &percent] {
        ui::hug(label);
    }
    ui::fixed_width(&percent, 54.0);
    percent.setAlignment(objc2_app_kit::NSTextAlignment::Right);
    let spacer = ui::spacer(mtm);
    let line = ui::row(mtm, 6.0, &[&swatch, &name, &spacer, &amount, &percent]);
    line.setAccessibilityElement(true);
    line.setAccessibilityLabel(Some(&ns(&tf(
        "{}: {} Tokens, {}",
        &[&row_name(row), &tokens(row.tokens), &part],
    ))));
    Retained::into_super(line)
}

const GROUPS: [ContextGroup; 6] = [
    ContextGroup::MemoryFile,
    ContextGroup::McpTool,
    ContextGroup::Agent,
    ContextGroup::Skill,
    ContextGroup::SystemTool,
    ContextGroup::Message,
];

/// A group's heading and its largest parts, with the rest counted.
fn group_views(mtm: MainThreadMarker, group: ContextGroup, parts: &[&ContextPart]) -> Vec<Retained<NSView>> {
    let heading = ui::label(
        mtm,
        group_name(group),
        &style::bold(11.0),
        &NSColor::secondaryLabelColor(),
    );
    let mut views = vec![ui::view(&heading)];
    let mut sorted = parts.to_vec();
    sorted.sort_by_key(|p| std::cmp::Reverse(p.tokens));
    for part in sorted.iter().take(PARTS_SHOWN) {
        let name = ui::label(mtm, &part_name(part), &style::small(), &NSColor::labelColor());
        name.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);
        name.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
        let amount = ui::label(
            mtm,
            &tokens(part.tokens),
            &style::small(),
            &NSColor::secondaryLabelColor(),
        );
        ui::hug(&amount);
        let spacer = ui::spacer(mtm);
        views.push(Retained::into_super(ui::row(mtm, 6.0, &[&name, &spacer, &amount])));
    }
    if sorted.len() > PARTS_SHOWN {
        let rest = (sorted.len() - PARTS_SHOWN).to_string();
        views.push(ui::view(&ui::label(
            mtm,
            &tf("und {} weitere", &[&rest]),
            &style::small(),
            &NSColor::tertiaryLabelColor(),
        )));
    }
    views
}

/// A row's name in the user's language where its id is known, else as the CLI words it.
fn row_name(row: &ContextRow) -> String {
    let known = match row.id.as_deref() {
        Some("system_prompt") => t("Systemprompt"),
        Some("system_tools") => t("Systemwerkzeuge"),
        Some("system_tools_deferred") => t("Systemwerkzeuge bei Bedarf"),
        Some("mcp_tools") => t("MCP-Werkzeuge"),
        Some("mcp_tools_deferred") => t("MCP-Werkzeuge bei Bedarf"),
        Some("mcp_instructions") => t("MCP-Anweisungen"),
        Some("custom_agents") => t("Eigene Agenten"),
        Some("memory_files") => t("Speicherdateien"),
        Some("skills") => t("Skills"),
        Some("messages") => t("Nachrichten"),
        Some("autocompact_buffer" | "compact_buffer") => t("Reserve fürs Verdichten"),
        Some("free_space") => t("Frei"),
        _ => return row.name.clone(),
    };
    known.to_string()
}

fn group_name(group: ContextGroup) -> &'static str {
    match group {
        ContextGroup::MemoryFile => t("Speicherdateien"),
        ContextGroup::McpTool => t("MCP-Werkzeuge"),
        ContextGroup::Agent => t("Eigene Agenten"),
        ContextGroup::Skill => t("Skills"),
        ContextGroup::SystemTool => t("Systemwerkzeuge"),
        ContextGroup::Message => t("Nachrichten nach Art"),
    }
}

fn part_name(part: &ContextPart) -> String {
    if part.group != ContextGroup::Message {
        return part.name.clone();
    }
    match part.name.as_str() {
        "tool_calls" => t("Werkzeugaufrufe"),
        "tool_results" => t("Werkzeugergebnisse"),
        "attachments" => t("Anhänge"),
        "assistant_messages" => t("Antworten"),
        "user_messages" => t("Eigene Nachrichten"),
        other => return other.to_string(),
    }
    .to_string()
}

/// The colour a row has in the bar and beside its name.
fn color_of(row: &ContextRow) -> Retained<NSColor> {
    match row.kind {
        ContextKind::Free => return NSColor::quaternaryLabelColor(),
        ContextKind::Buffer => return NSColor::tertiaryLabelColor(),
        ContextKind::Deferred => return NSColor::separatorColor(),
        ContextKind::Used => {}
    }
    match row.id.as_deref() {
        Some("system_prompt") => NSColor::systemGrayColor(),
        Some("system_tools") => NSColor::systemBlueColor(),
        Some("mcp_tools" | "mcp_instructions") => NSColor::systemTealColor(),
        Some("custom_agents") => NSColor::systemPurpleColor(),
        Some("memory_files") => NSColor::systemPinkColor(),
        Some("skills") => NSColor::systemYellowColor(),
        Some("messages") => NSColor::systemGreenColor(),
        _ => NSColor::systemBrownColor(),
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `TopDown` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentContextColumn"]
    pub(crate) struct TopDown;

    impl TopDown {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

pub(crate) struct BarIvars {
    /// Each segment's share of the window and colour.
    segments: Vec<(f64, Retained<NSColor>)>,
    /// Where compacting by itself starts, as a share of the window.
    mark: Option<f64>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Bar` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentContextBar"]
    #[ivars = BarIvars]
    pub(crate) struct Bar;

    unsafe impl NSObjectProtocol for Bar {}

    impl Bar {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }
    }
);

impl Bar {
    fn new(mtm: MainThreadMarker, breakdown: &ContextBreakdown) -> Retained<Bar> {
        let size = breakdown.size.max(1) as f64;
        let segments = breakdown
            .rows
            .iter()
            .filter(|r| r.kind != ContextKind::Deferred && r.tokens > 0)
            .map(|r| (r.tokens as f64 / size, color_of(r)))
            .collect();
        let mark = breakdown.auto_compact_at.map(|at| at as f64 / size);
        let this = Bar::alloc(mtm).set_ivars(BarIvars { segments, mark });
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let bar: Retained<Bar> = unsafe { msg_send![super(this), init] };
        ui::fixed_height(&bar, 10.0);
        bar.setAccessibilityElement(false);
        bar
    }

    fn draw(&self) {
        let bounds = self.bounds();
        let clip = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 3.0, 3.0);
        NSColor::quaternaryLabelColor().setFill();
        clip.fill();
        clip.addClip();
        let mut x = bounds.origin.x;
        for (share, color) in &self.ivars().segments {
            let width = (share * bounds.size.width).max(0.0);
            color.setFill();
            objc2_app_kit::NSRectFill(NSRect::new(
                NSPoint::new(x, bounds.origin.y),
                NSSize::new(width, bounds.size.height),
            ));
            x += width;
        }
        if let Some(mark) = self.ivars().mark.filter(|m| (0.0..=1.0).contains(m)) {
            NSColor::labelColor().setFill();
            objc2_app_kit::NSRectFill(NSRect::new(
                NSPoint::new(bounds.origin.x + mark * bounds.size.width - 0.5, bounds.origin.y),
                NSSize::new(1.0, bounds.size.height),
            ));
        }
    }
}

pub(crate) struct SwatchIvars {
    color: Retained<NSColor>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Swatch` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentContextSwatch"]
    #[ivars = SwatchIvars]
    pub(crate) struct Swatch;

    unsafe impl NSObjectProtocol for Swatch {}

    impl Swatch {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || {
                self.ivars().color.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 2.0, 2.0).fill();
            });
        }
    }
);

impl Swatch {
    fn new(mtm: MainThreadMarker, color: &NSColor) -> Retained<Swatch> {
        let this = Swatch::alloc(mtm).set_ivars(SwatchIvars { color: color.retain() });
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let swatch: Retained<Swatch> = unsafe { msg_send![super(this), init] };
        ui::fixed_width(&swatch, 9.0);
        ui::fixed_height(&swatch, 9.0);
        swatch
    }
}

/// The agent's last token report for the left of the status line, where other agents show the context, with its
/// breakdown as a tooltip; `None` without a report.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn token_text(usage: &Usage) -> Option<(String, String)> {
    let report = usage.reported_tokens.as_ref()?;
    let count = tf("{} Tokens", &[&tokens(report.total)]);
    let mut tip = vec![
        tf("Zuletzt gemeldet: {}", &[&count]),
        tf("Eingabe: {}", &[&tokens(report.input)]),
        tf("Ausgabe: {}", &[&tokens(report.output)]),
    ];
    if let Some(read) = report.cache_read {
        tip.push(tf("Cache gelesen: {}", &[&tokens(read)]));
    }
    if let Some(write) = report.cache_write {
        tip.push(tf("Cache geschrieben: {}", &[&tokens(write)]));
    }
    if let Some(thought) = report.thought {
        tip.push(tf("Reasoning: {}", &[&tokens(thought)]));
    }
    Some((count, tip.join("\n")))
}

/// The status line's cost: the session's, and the last turn's when it is known; with the details as a tooltip.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn cost_text(usage: &Usage) -> (String, Option<String>) {
    if let Some(cost) = usage.reported_cost.as_ref().filter(|c| c.currency != "USD") {
        let text = super::format::reported_cost(cost);
        let tip = tf("Sitzung: {}", &[&text]);
        return (text, Some(tip));
    }
    let Some(session) = usage.cost_usd else {
        return (String::new(), None);
    };
    let turn = usage.last_turn();
    let text = match turn.and_then(|t| t.cost_usd) {
        Some(cost) => tf("Sitzung {} · Zug {}", &[&amount(session), &amount(cost)]),
        None => amount(session),
    };
    let mut tip = vec![tf("Sitzung: {}", &[&amount(session)])];
    if let Some(turn) = turn {
        let cost = turn.cost_usd.map_or_else(|| t("unbekannt").to_string(), amount);
        let mut line = tf("Letzter Zug: {}", &[&cost]);
        if let Some(count) = turn.tokens.map(|t| t.total()) {
            line.push_str(&format!(" · {}", tf("{} Tokens", &[&tokens(count)])));
        }
        if let Some(duration) = turn.duration {
            line.push_str(&format!(" · {}", elapsed(duration)));
        }
        tip.push(line);
    }
    if usage.models.len() > 1 {
        tip.extend(
            usage
                .models
                .iter()
                .map(|m| format!("{}: {}", m.model, amount(m.cost_usd))),
        );
    }
    (text, Some(tip.join("\n")))
}

/// What the status line says about spending: on a subscription the plan's usage windows instead of dollars, else
/// the cost.
pub(crate) fn spend_text(thread: &term_thread::Thread) -> (String, Option<String>) {
    let usage = thread.usage();
    if !usage.subscription {
        return cost_text(usage);
    }
    let windows = super::usage::windows(thread);
    let mut text = vec![t("Abo").to_string()];
    text.extend(
        windows
            .iter()
            .map(|w| format!("{} {}", super::cards::window_name(&w.name), percent(w.used))),
    );
    let mut tip = vec![t("Abo: Die Nutzung zählt gegen die Limits des Plans, nicht nach Tokens.").to_string()];
    tip.extend(windows.iter().filter_map(|w| {
        Some(tf(
            "{}: zurückgesetzt {}",
            &[&super::cards::window_name(&w.name), w.resets.as_ref()?],
        ))
    }));
    (text.join(" · "), Some(tip.join("\n")))
}
