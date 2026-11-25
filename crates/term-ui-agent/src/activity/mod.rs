//! The activity view of one session, which the buttons in the head of its tab and `/tasks` open: a popover listing
//! its subagents, its background work or both with the session itself, each with what it does now, Show and Stop,
//! kept current while it shows.

use std::cell::{Cell, RefCell};
use std::time::{Duration, SystemTime};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSColor, NSLayoutAttribute, NSPopover, NSPopoverBehavior, NSScrollView, NSStackView,
    NSView, NSViewController,
};
use objc2_foundation::{NSObjectProtocol, NSSize};
use splat_i18n::t;
use term_thread::{BackgroundKind, Status, TaskStatus};

use crate::AgentKey;
use crate::common::util::{guarded, main_after, ns, pin_edges, set_action, set_label, symbol};
use crate::stop::Stop;
use crate::tab::AgentTab;
use crate::work::Part;

#[cfg(feature = "selftest")]
mod selftest_probe;
mod session;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

const WIDTH: f64 = 420.0;

/// What a button of the popover does.
#[derive(Clone, PartialEq)]
enum Action {
    ShowTab(AgentKey),
    StopTurn(AgentKey),
    Reveal(AgentKey, String),
    StopTask(AgentKey, String),
    ShowOutput(AgentKey, String),
}

pub(crate) struct ActivityIvars {
    popover: RefCell<Option<Retained<NSPopover>>>,
    actions: RefCell<Vec<Action>>,
    scheduled: Cell<bool>,
    /// The tab whose work the popover shows, and which of it.
    only: Cell<Option<(AgentKey, Part)>>,
    /// The rows the popover shows, to change their texts in place.
    shown: RefCell<Option<Shown>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Activity` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentActivity"]
    #[ivars = ActivityIvars]
    pub(crate) struct Activity;

    unsafe impl NSObjectProtocol for Activity {}

    impl Activity {
        #[unsafe(method(ideActivityAction:))]
        fn action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = sender.and_then(|s| s.downcast_ref::<NSButton>()).map_or(-1, |b| b.tag());
                let action = usize::try_from(tag).ok().and_then(|i| self.ivars().actions.borrow().get(i).cloned());
                if let Some(action) = action {
                    self.perform(action);
                }
            });
        }
    }
);

/// The app's activity popover, made as it first opens.
#[derive(Default)]
pub(crate) struct Activities {
    activity: RefCell<Option<Retained<Activity>>>,
}

fn of() -> Option<Retained<Activity>> {
    crate::host::state().activity.activity.borrow().clone()
}

fn made(mtm: MainThreadMarker) -> Retained<Activity> {
    if let Some(activity) = of() {
        return activity;
    }
    let ivars = ActivityIvars {
        popover: RefCell::new(None),
        actions: RefCell::new(Vec::new()),
        scheduled: Cell::new(false),
        only: Cell::new(None),
        shown: RefCell::new(None),
    };
    // SAFETY: NSObject's initializer, called once on the newly allocated object.
    let activity: Retained<Activity> = unsafe { msg_send![super(Activity::alloc(mtm).set_ivars(ivars)), init] };
    *crate::host::state().activity.activity.borrow_mut() = Some(activity.clone());
    activity
}

/// Opens the list of `part` of the session in tab `key` below `anchor`.
pub(crate) fn show(key: AgentKey, part: Part, anchor: &NSView) {
    let activity = made(anchor.mtm());
    activity.ivars().only.set(Some((key, part)));
    activity.open(anchor);
}

/// Tells the popover that the session in tab `key` changed, while it shows that session's work.
pub(crate) fn changed(key: AgentKey) {
    if let Some(activity) = of()
        && activity.ivars().only.get().is_some_and(|(shown, _)| shown == key)
    {
        activity.changed();
    }
}

/// How a session stands.
fn session_status(status: Status) -> &'static str {
    match status {
        Status::Idle => t("Bereit"),
        Status::Running => t("Arbeitet"),
        Status::RequiresAction => t("Wartet auf dich"),
    }
}

/// How a task stands.
fn task_status(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => t("Wartet"),
        TaskStatus::Running => t("Läuft"),
        TaskStatus::Paused => t("Pausiert"),
        TaskStatus::Completed => t("Fertig"),
        TaskStatus::Failed => t("Fehlgeschlagen"),
        TaskStatus::Stopped => t("Gestoppt"),
    }
}

fn background_symbol(kind: &BackgroundKind) -> &'static str {
    match kind {
        BackgroundKind::Shell => "terminal",
        BackgroundKind::Monitor => "eye",
        BackgroundKind::Subagent => "person.2",
        BackgroundKind::Workflow => "flowchart",
        BackgroundKind::Other(_) => "gearshape.2",
    }
}

/// One line of the popover.
struct Line {
    symbol: &'static str,
    tint: Retained<NSColor>,
    title: String,
    detail: String,
    indent: bool,
    buttons: Vec<(&'static str, Action)>,
}

/// What decides the popover's rows: its sections and, per line, its symbol, indent and buttons. While it stays, the
/// rows keep their views and only their texts change, so the keyboard focus and VoiceOver's place stay where they are.
#[derive(PartialEq)]
enum Shape {
    Section(&'static str),
    Line {
        symbol: &'static str,
        indent: bool,
        buttons: Vec<(&'static str, Action)>,
    },
}

fn shape_of(sections: &[(&'static str, Vec<Line>)]) -> Vec<Shape> {
    let mut shape = Vec::new();
    for (title, lines) in sections.iter().filter(|(_, lines)| !lines.is_empty()) {
        shape.push(Shape::Section(title));
        shape.extend(lines.iter().map(|line| Shape::Line {
            symbol: line.symbol,
            indent: line.indent,
            buttons: line.buttons.clone(),
        }));
    }
    shape
}

/// The views of a line whose texts change in place.
struct LineViews {
    icon: Retained<objc2_app_kit::NSImageView>,
    title: Retained<objc2_app_kit::NSTextField>,
    detail: Retained<objc2_app_kit::NSTextField>,
    buttons: Vec<(Retained<NSButton>, &'static str)>,
}

impl LineViews {
    fn update(&self, line: &Line) {
        if self.title.stringValue().to_string() != line.title {
            self.title.setStringValue(&ns(&line.title));
            self.icon.setImage(symbol(line.symbol, &line.title).as_deref());
            for (button, title) in &self.buttons {
                set_label(button, Some(&format!("{title}: {}", line.title)));
            }
        }
        if self.detail.stringValue().to_string() != line.detail {
            self.detail.setStringValue(&ns(&line.detail));
        }
        self.icon.setContentTintColor(Some(&line.tint));
    }
}

/// The popover's rows and what they were built from.
struct Shown {
    shape: Vec<Shape>,
    lines: Vec<LineViews>,
}

impl Shown {
    /// What tells the rows apart, in their order: the action of each one's first button, as a row keeps it while it
    /// shows.
    fn identities(&self) -> Vec<Option<Action>> {
        self.shape
            .iter()
            .filter_map(|shape| match shape {
                Shape::Line { buttons, .. } => Some(buttons.first().map(|(_, action)| action.clone())),
                Shape::Section(_) => None,
            })
            .collect()
    }

    /// The row and the button of it with the keyboard focus in `window`.
    fn focused(&self, window: &objc2_app_kit::NSWindow) -> Option<(usize, usize)> {
        let focus = window.firstResponder()?;
        let focus: &AnyObject = focus.as_ref();
        self.lines.iter().enumerate().find_map(|(row, line)| {
            line.buttons
                .iter()
                .position(|(button, _)| {
                    let button: &AnyObject = button.as_ref();
                    std::ptr::eq(button, focus)
                })
                .map(|at| (row, at))
        })
    }
}

/// The place in `new` of row `row` of `old`, or of the nearest row after it that is still there, else of the nearest
/// before it.
fn same_or_next<T: PartialEq>(old: &[T], row: usize, new: &[T]) -> Option<usize> {
    let place = |item: &T| new.iter().position(|other| other == item);
    old.get(row..)
        .into_iter()
        .flatten()
        .find_map(place)
        .or_else(|| old.get(..row).into_iter().flatten().rev().find_map(place))
}

/// The lines of the session in `tab`: the session, all its subagents and its background work that runs.
fn agent_lines(tab: &AgentTab, now: SystemTime) -> Vec<Line> {
    let thread = tab.view_snapshot();
    let mut buttons = vec![(t("Zeigen"), Action::ShowTab(tab.key))];
    if thread.status() != Status::Idle {
        buttons.push((t("Stopp"), Action::StopTurn(tab.key)));
    }
    let tint = match thread.status() {
        Status::Running => NSColor::controlAccentColor(),
        Status::RequiresAction => NSColor::systemOrangeColor(),
        Status::Idle => NSColor::tertiaryLabelColor(),
    };
    let mut lines = vec![Line {
        symbol: "sparkle",
        tint,
        title: tab.title(),
        detail: session_status(thread.status()).to_string(),
        indent: false,
        buttons,
    }];
    lines.extend(
        thread
            .subagents()
            .iter()
            .map(|sub| session::subagent_line(tab.key, sub, now, true)),
    );
    let background = thread
        .background()
        .iter()
        .filter(|b| !b.status.is_final() && Part::All.holds(&b.kind));
    lines.extend(background.map(|item| session::background_line(tab.key, item, now, true)));
    lines
}

/// The lines of `part` of the session in tab `key`.
fn session_lines(key: AgentKey, part: Part, now: SystemTime) -> Vec<Line> {
    let Some(tab) = crate::get(key) else {
        return Vec::new();
    };
    match part {
        Part::All => agent_lines(&tab, now),
        Part::Subagents | Part::Background => session::lines(key, &tab.view_snapshot(), part, now),
    }
}

impl Activity {
    fn is_shown(&self) -> bool {
        self.ivars().popover.borrow().as_ref().is_some_and(|p| p.isShown())
    }

    /// Updates the popover soon, if it shows.
    fn changed(&self) {
        if self.is_shown() && !self.ivars().scheduled.replace(true) {
            main_after(Duration::from_millis(300), || {
                if let Some(activity) = of() {
                    activity.ivars().scheduled.set(false);
                    activity.fill();
                }
            });
        }
    }

    /// Opens the popover below `anchor`.
    fn open(&self, anchor: &NSView) {
        let mtm = self.mtm();
        let popover = self
            .ivars()
            .popover
            .borrow_mut()
            .get_or_insert_with(|| {
                let popover = NSPopover::new(mtm);
                popover.setBehavior(NSPopoverBehavior::Transient);
                popover.setContentViewController(Some(&NSViewController::new(mtm)));
                popover
            })
            .clone();
        self.fill();
        popover.showRelativeToRect_ofView_preferredEdge(anchor.bounds(), anchor, objc2_foundation::NSRectEdge::MaxY);
        relink(&popover);
        self.tick();
    }

    /// Keeps the running times of an open popover current.
    fn tick(&self) {
        main_after(Duration::from_secs(1), || {
            if let Some(activity) = of()
                && activity.is_shown()
            {
                activity.fill();
                activity.tick();
            }
        });
    }

    /// The session's list the popover shows.
    fn sections(&self) -> Vec<(&'static str, Vec<Line>)> {
        let Some((key, part)) = self.ivars().only.get() else {
            return Vec::new();
        };
        vec![(part.heading(), session_lines(key, part, SystemTime::now()))]
    }

    /// Shows what runs now: in the rows there are, while the same rows would show, else in rows built anew.
    fn fill(&self) {
        let sections = self.sections();
        let shape = shape_of(&sections);
        if let Some(shown) = self.ivars().shown.borrow().as_ref()
            && shown.shape == shape
        {
            let lines = sections.iter().flat_map(|(_, lines)| lines);
            for (views, line) in shown.lines.iter().zip(lines) {
                views.update(line);
            }
            #[cfg(feature = "selftest")]
            UPDATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        self.build(self.mtm(), &sections, shape);
    }

    /// Builds the popover's rows anew.
    fn build(&self, mtm: MainThreadMarker, sections: &[(&'static str, Vec<Line>)], shape: Vec<Shape>) {
        let mut actions = Vec::new();
        let mut shown = Vec::new();
        let stack = NSStackView::new(mtm);
        stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(crate::style::space::XS);
        stack.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 12.0,
            left: 12.0,
            bottom: 12.0,
            right: 12.0,
        });
        for (title, lines) in sections.iter().filter(|(_, lines)| !lines.is_empty()) {
            let heading = crate::agent_panel::section_label(mtm, title);
            stack.addArrangedSubview(&heading);
            for line in lines {
                let (view, views) = self.line_view(mtm, line, &mut actions);
                shown.push(views);
                stack.addArrangedSubview(&view);
                view.setTranslatesAutoresizingMaskIntoConstraints(false);
                view.widthAnchor()
                    .constraintEqualToConstant(WIDTH - 24.0)
                    .setActive(true);
            }
            if let Some(last) = stack.arrangedSubviews().lastObject() {
                stack.setCustomSpacing_afterView(crate::style::space::XL, &last);
            }
        }
        if shape.is_empty() {
            let label = objc2_app_kit::NSTextField::labelWithString(&ns(t("Gerade läuft nichts.")), mtm);
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            stack.addArrangedSubview(&label);
        }
        *self.ivars().actions.borrow_mut() = actions;
        let focused = self.focused_row();
        *self.ivars().shown.borrow_mut() = Some(Shown { shape, lines: shown });
        self.show_rows(mtm, &stack);
        if let Some(focused) = focused {
            self.refocus(focused);
        }
    }

    fn window(&self) -> Option<Retained<objc2_app_kit::NSWindow>> {
        let popover = self.ivars().popover.borrow();
        popover.as_ref()?.contentViewController()?.view().window()
    }

    /// The rows as they show, with the row and the button of it with the keyboard focus.
    fn focused_row(&self) -> Option<(Vec<Option<Action>>, usize, usize)> {
        let window = self.window()?;
        let shown = self.ivars().shown.borrow();
        let shown = shown.as_ref()?;
        let (row, button) = shown.focused(&window)?;
        Some((shown.identities(), row, button))
    }

    /// Gives the keyboard focus back to the row it was on before the rows were built anew, by what the row is, else
    /// to the nearest row after it: the popover never sends the focus back to its start.
    fn refocus(&self, (before, row, button): (Vec<Option<Action>>, usize, usize)) {
        let Some(window) = self.window() else { return };
        let shown = self.ivars().shown.borrow();
        let Some(shown) = shown.as_ref() else { return };
        let Some(line) = same_or_next(&before, row, &shown.identities()).and_then(|at| shown.lines.get(at)) else {
            return;
        };
        if let Some((button, _)) = line.buttons.get(button).or_else(|| line.buttons.last()) {
            window.makeFirstResponder(Some(button));
        }
    }

    /// Puts `stack` into the popover, scrolling beyond its greatest height.
    fn show_rows(&self, mtm: MainThreadMarker, stack: &NSStackView) {
        let height = stack.fittingSize().height.min(520.0);
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let document = crate::agent_panel::flipped(mtm);
        document.addSubview(stack);
        pin_edges(stack, &document);
        scroll.setDocumentView(Some(&document));
        document.setTranslatesAutoresizingMaskIntoConstraints(false);
        let clip = scroll.contentView();
        document
            .widthAnchor()
            .constraintEqualToAnchor(&clip.widthAnchor())
            .setActive(true);
        let size = NSSize::new(WIDTH, height.max(60.0));
        scroll.setFrameSize(size);
        if let Some(popover) = self.ivars().popover.borrow().as_ref()
            && let Some(controller) = popover.contentViewController()
        {
            controller.setView(&scroll);
            // A popover takes its size from its view controller, not from its own content size.
            controller.setPreferredContentSize(size);
            popover.setContentSize(size);
            relink(popover);
        }
        scroll.setAccessibilityLabel(Some(&ns(t("Aktivität"))));
    }

    fn line_view(
        &self,
        mtm: MainThreadMarker,
        line: &Line,
        actions: &mut Vec<Action>,
    ) -> (Retained<NSView>, LineViews) {
        let icon = objc2_app_kit::NSImageView::new(mtm);
        icon.setImage(symbol(line.symbol, &line.title).as_deref());
        icon.setContentTintColor(Some(&line.tint));
        icon.setContentHuggingPriority_forOrientation(750.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
        let title = objc2_app_kit::NSTextField::labelWithString(&ns(&line.title), mtm);
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        title.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(crate::style::LABEL)));
        let detail = objc2_app_kit::NSTextField::labelWithString(&ns(&line.detail), mtm);
        detail.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        detail.setFont(Some(&crate::style::small()));
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        for label in [&title, &detail] {
            label.setContentCompressionResistancePriority_forOrientation(
                250.0,
                objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
            );
        }
        let texts = NSStackView::new(mtm);
        texts.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        texts.setAlignment(NSLayoutAttribute::Leading);
        texts.setSpacing(crate::style::space::HAIR);
        texts.addArrangedSubview(&title);
        texts.addArrangedSubview(&detail);
        let row = NSStackView::new(mtm);
        row.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(crate::style::space::S);
        row.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 2.0,
            left: if line.indent { 18.0 } else { 0.0 },
            bottom: 2.0,
            right: 0.0,
        });
        row.addArrangedSubview(&icon);
        row.addArrangedSubview(&texts);
        let spacer = NSView::new(mtm);
        spacer.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
        row.addArrangedSubview(&spacer);
        let mut buttons = Vec::new();
        for (title, action) in &line.buttons {
            // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
            let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
            set_action(&button, self, sel!(ideActivityAction:));
            button.setTag(isize::try_from(actions.len()).unwrap_or(-1));
            button.setControlSize(objc2_app_kit::NSControlSize::Small);
            button.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
            button.setFont(Some(&crate::style::small()));
            set_label(&button, Some(&format!("{title}: {}", line.title)));
            actions.push(action.clone());
            row.addArrangedSubview(&button);
            buttons.push((button, *title));
        }
        row.setAccessibilityElement(false);
        let views = LineViews {
            icon,
            title,
            detail,
            buttons,
        };
        (Retained::into_super(row), views)
    }

    fn perform(&self, action: Action) {
        match action {
            Action::ShowTab(key) => {
                if let Some(tab) = crate::get(key) {
                    tab.select();
                }
            }
            Action::StopTurn(key) => {
                if let Some(tab) = crate::get(key) {
                    tab.stop_turn(Stop::Activity);
                }
            }
            Action::Reveal(key, sub) => {
                if let Some(tab) = crate::get(key) {
                    tab.select();
                    tab.timeline().reveal(&sub);
                }
            }
            Action::StopTask(key, task) => {
                if let Some(tab) = crate::get(key) {
                    tab.stop_task(&task);
                }
            }
            Action::ShowOutput(key, task) => {
                if let Some(tab) = crate::get(key) {
                    tab.show_background_output(&task);
                }
            }
        }
        self.changed();
    }
}

/// Links the buttons of a shown popover into its key view loop: AppKit makes the loop once, when the popover opens,
/// and the rows are built anew each second.
fn relink(popover: &NSPopover) {
    if let Some(window) = popover.contentViewController().and_then(|c| c.view().window()) {
        window.recalculateKeyViewLoop();
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundKind, TaskStatus, background_symbol, same_or_next, task_status};

    #[test]
    fn the_focus_keeps_its_row_else_takes_the_next_else_the_one_before() {
        let old = ["session", "subagent", "shell", "server"];
        assert_eq!(same_or_next(&old, 2, &["session", "shell", "server"]), Some(1));
        assert_eq!(same_or_next(&old, 1, &["session", "shell", "server"]), Some(1));
        assert_eq!(same_or_next(&old, 1, &["server", "session"]), Some(0));
        assert_eq!(same_or_next(&old, 3, &["session", "subagent"]), Some(1));
        assert_eq!(same_or_next(&old, 0, &["new"]), None);
    }

    #[test]
    fn every_task_status_has_words_of_its_own() {
        let all = [
            TaskStatus::Pending,
            TaskStatus::Running,
            TaskStatus::Paused,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Stopped,
        ];
        let words: std::collections::BTreeSet<&str> = all.into_iter().map(task_status).collect();
        assert_eq!(words.len(), all.len());
        assert_eq!(task_status(TaskStatus::Failed), "Fehlgeschlagen");
    }

    #[test]
    fn every_kind_of_background_work_has_a_symbol_of_its_own() {
        let kinds = [
            BackgroundKind::Shell,
            BackgroundKind::Monitor,
            BackgroundKind::Subagent,
            BackgroundKind::Workflow,
            BackgroundKind::Other("cron".into()),
        ];
        let symbols: std::collections::BTreeSet<&str> = kinds.iter().map(background_symbol).collect();
        assert_eq!(symbols.len(), kinds.len());
        assert_eq!(
            background_symbol(&BackgroundKind::Other("anything".into())),
            "gearshape.2"
        );
    }
}
