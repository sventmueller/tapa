//! The views of the Git panel: branch and remote controls, progress, problems, the commit message and the list.

use objc2::rc::{Retained, Weak};
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAccessibility, NSBezelStyle, NSBorderType, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSFont,
    NSImageView, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLineBreakMode, NSMenu, NSPopUpButton,
    NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSStackView, NSStackViewDistribution,
    NSStackViewGravity, NSTextAlignment, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize};
use splat_i18n::t;

use super::commit_field::CommitTextView;
use super::model::Phase;
use super::panel::GitPanel;
use super::rows::GitOutlineView;
use super::text::Problem;
use term_ui::kit::panel::{header_button, header_row};
use term_ui::kit::ui_size::{self, Aspect};
use term_ui::kit::{activate, borderless_button, label, ns, pin_edges, pin_sides, set_action, symbol};

/// How far the panel's content keeps from the sidebar's edges: as far as the panel's title.
pub(super) const MARGIN: f64 = 8.0;
const MESSAGE_HEIGHT: f64 = 62.0;
/// The room beside the text of a problem: its symbol, the button that closes it and the spacing around them.
const PROBLEM_INSET: f64 = 52.0;
/// The room the conflict banner keeps around its text.
pub(crate) const BANNER_INSET: f64 = 16.0;
/// How far a borderless button sets its title in from its own edge.
const LINK_BEARING: f64 = 2.0;
pub(crate) struct Ui {
    pub(crate) page: Retained<NSView>,
    /// The ahead/behind count with fetch, pull and push, at the end of the panel's title row.
    pub(crate) header: Retained<NSStackView>,
    empty: Retained<NSView>,
    content: Retained<NSStackView>,
    pub(crate) branch: Retained<NSPopUpButton>,
    pub(crate) distance: Retained<NSTextField>,
    pub(crate) fetch: Retained<NSButton>,
    pub(crate) pull: Retained<NSButton>,
    pub(crate) push: Retained<NSButton>,
    pub(crate) progress_row: Retained<NSStackView>,
    pub(crate) progress: Retained<NSProgressIndicator>,
    pub(crate) progress_label: Retained<NSTextField>,
    pub(crate) cancel: Retained<NSButton>,
    pub(crate) banner: Banner,
    /// The note that git runs neutralised, while the user does not trust the workspace.
    pub(crate) untrusted: Retained<NSView>,
    problem_row: Retained<NSStackView>,
    problem_label: Retained<NSTextField>,
    problem_output: Retained<NSButton>,
    pub(crate) message: Retained<CommitTextView>,
    pub(crate) placeholder: Retained<NSTextField>,
    pub(crate) amend: Retained<NSButton>,
    pub(crate) commit: Retained<NSButton>,
    pub(crate) outline: Retained<GitOutlineView>,
}

impl Ui {
    pub(crate) fn new(mtm: MainThreadMarker, panel: &GitPanel) -> Ui {
        let page: Retained<NSView> = super::panel::GitPage::new(mtm, panel.id()).into_super();
        let empty = NSView::new(mtm);
        let content = NSStackView::new(mtm);
        content.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        content.setAlignment(NSLayoutAttribute::Leading);
        content.setDistribution(NSStackViewDistribution::Fill);
        ui_size::follow_spacing(&content, term_ui::kit::style::space::M);
        for view in [&*empty, &content as &NSView] {
            page.addSubview(view);
            pin_edges(view, &page);
        }
        let (header, distance, [fetch, pull, push]) = header(mtm, panel);
        let (branch_row, branch) = branch_row(mtm);
        let untrusted = untrusted_row(mtm, panel);
        let (progress_row, progress, progress_label, cancel) = progress_row(mtm, panel);
        let banner = Banner::new(mtm, panel);
        let (problem_row, problem_label, problem_output) = problem_row(mtm, panel);
        let (message_box, message, placeholder) = message_box(mtm, panel);
        let (commit_row, amend, commit) = commit_row(mtm, panel);
        let (list, outline) = outline(mtm, panel);
        for (view, inset) in [
            (&*branch_row as &NSView, MARGIN),
            (&untrusted, MARGIN),
            (&progress_row, MARGIN),
            (&banner.view, MARGIN),
            (&problem_row, MARGIN),
            (&message_box, MARGIN),
            (&commit_row, MARGIN),
            (&list, 0.0),
        ] {
            content.addArrangedSubview(view);
            pin_sides(view, &content, inset);
            view.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Vertical);
        }
        list.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
        follow_wrap_width(&problem_label, &page, PROBLEM_INSET);
        follow_wrap_width(&banner.detail, &page, BANNER_INSET);
        progress_row.setHidden(true);
        banner.view.setHidden(true);
        problem_row.setHidden(true);
        untrusted.setHidden(true);
        content.setHidden(true);
        Ui {
            page,
            header,
            empty,
            content,
            branch,
            distance,
            fetch,
            pull,
            push,
            progress_row,
            progress,
            progress_label,
            cancel,
            banner,
            untrusted,
            problem_row,
            problem_label,
            problem_output,
            message,
            placeholder,
            amend,
            commit,
            outline,
        }
    }

    /// Shows the repository's controls when it is ready, else an explanation of why not and what helps.
    pub(crate) fn show_phase(&self, panel: &GitPanel, phase: &Phase) {
        let mtm = panel.mtm();
        self.content.setHidden(*phase != Phase::Ready);
        self.empty.setHidden(*phase == Phase::Ready);
        for subview in self.empty.subviews().iter() {
            subview.removeFromSuperview();
        }
        let missing = super::text::problem(&term_git::Error::GitMissing).message;
        let (symbol_name, title, detail, button): (&str, &str, &str, Option<(&str, Sel)>) = match phase {
            Phase::Ready => return,
            Phase::Loading => (
                "arrow.triangle.branch",
                t("Git"),
                t("Das Repository wird gelesen …"),
                None,
            ),
            Phase::NoGit => ("exclamationmark.triangle", t("Git fehlt"), &missing, None),
            Phase::NotRepository => (
                "arrow.triangle.branch",
                t("Kein Git-Repository"),
                t("Dieser Ordner wird noch nicht mit Git versioniert."),
                Some((t("Repository anlegen"), sel!(termGitInit:))),
            ),
            Phase::Failed(message) => (
                "exclamationmark.triangle",
                t("Git meldet einen Fehler"),
                message.as_str(),
                Some((t("Erneut versuchen"), sel!(termGitRetry:))),
            ),
        };
        let state = empty_state(mtm, panel, symbol_name, title, detail, button);
        self.empty.addSubview(&state);
        state.setTranslatesAutoresizingMaskIntoConstraints(false);
        let raised = state
            .centerYAnchor()
            .constraintEqualToAnchor_constant(&self.empty.centerYAnchor(), -40.0);
        ui_size::follow_length(&raised, -40.0);
        activate([
            state
                .centerXAnchor()
                .constraintEqualToAnchor(&self.empty.centerXAnchor()),
            raised,
            state
                .leadingAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&self.empty.leadingAnchor(), 16.0),
            state
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&self.empty.trailingAnchor(), -16.0),
        ]);
    }

    /// The width a wrapping text in a row of the panel has, inside the margins and the space of `inset`.
    pub(crate) fn text_width(&self, inset: f64) -> f64 {
        text_width(&self.page, inset)
    }

    /// Shows `problem` above the commit message, or hides the row.
    pub(crate) fn show_problem(&self, problem: Option<&Problem>) {
        let Some(problem) = problem else {
            self.problem_row.setHidden(true);
            return;
        };
        self.problem_label
            .setPreferredMaxLayoutWidth(self.text_width(PROBLEM_INSET));
        self.problem_label.setStringValue(&ns(&problem.message));
        self.problem_output.setHidden(problem.output.is_none());
        self.problem_output.setTitle(&ns(if problem.hook {
            t("Ausgabe des Hooks …")
        } else {
            t("Details …")
        }));
        self.problem_row.setHidden(false);
    }
}

/// The width a wrapping text in a row of `page` has, inside the margins and the space `inset` takes at the interface
/// size.
fn text_width(page: &NSView, inset: f64) -> f64 {
    (page.frame().size.width - 2.0 * MARGIN - ui_size::scaled(inset)).max(120.0)
}

/// Keeps the width `label` wraps at, once it was given one, fitting `page` at the interface size.
fn follow_wrap_width(label: &NSTextField, page: &NSView, inset: f64) {
    let page = Weak::from(page);
    ui_size::keep(label, Aspect::Layout, move |label| {
        if let Some(page) = page.load()
            && label.preferredMaxLayoutWidth() > 0.0
        {
            label.setPreferredMaxLayoutWidth(text_width(&page, inset));
        }
    });
}

/// The note that a merge or rebase stopped at conflicts, with what carries it on or abandons it.
pub(crate) struct Banner {
    pub(crate) view: Retained<NSBox>,
    pub(crate) title: Retained<NSTextField>,
    pub(crate) detail: Retained<NSTextField>,
    pub(crate) resume: Retained<NSButton>,
    pub(crate) abort: Retained<NSButton>,
}

impl Banner {
    fn new(mtm: MainThreadMarker, panel: &GitPanel) -> Banner {
        let view = NSBox::new(mtm);
        view.setBoxType(NSBoxType::Custom);
        view.setTitlePosition(NSTitlePosition::NoTitle);
        view.setBorderWidth(0.0);
        view.setFillColor(&NSColor::systemOrangeColor().colorWithAlphaComponent(0.16));
        ui_size::keep(&*view, Aspect::Layout, |view| {
            view.setCornerRadius(ui_size::scaled(6.0));
            view.setContentViewMargins(NSSize::new(ui_size::scaled(BANNER_INSET / 2.0), ui_size::scaled(6.0)));
        });
        let title = label(mtm, "");
        ui_size::set_font(&title, &NSFont::boldSystemFontOfSize(NSFont::smallSystemFontSize()));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let detail = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        ui_size::set_font(&detail, &term_ui::kit::style::small_system());
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        // Text in the sidebar never makes it wider; it wraps or is cut instead.
        for text in [&title, &detail] {
            text.setContentCompressionResistancePriority_forOrientation(
                100.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
        }
        let button = |title: &str, action: Sel| {
            // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
            let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
            set_action(&button, panel, action);
            button.setBezelStyle(NSBezelStyle::Push);
            ui_size::follow_control_size(&button, NSControlSize::Small);
            ui_size::set_font(&button, &term_ui::kit::style::small_system());
            button.setContentCompressionResistancePriority_forOrientation(
                100.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
            button
        };
        let resume = button(t("Fortsetzen"), sel!(termGitContinue:));
        let abort = button(t("Vorgang abbrechen …"), sel!(termGitAbort:));
        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        ui_size::follow_spacing(&buttons, term_ui::kit::style::space::S);
        buttons.addArrangedSubview(&resume);
        buttons.addArrangedSubview(&abort);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        ui_size::follow_spacing(&stack, term_ui::kit::style::space::XS);
        for row in [&*title as &NSView, &detail, &buttons] {
            stack.addArrangedSubview(row);
        }
        if let Some(content) = view.contentView() {
            content.addSubview(&stack);
            pin_edges(&stack, &content);
        }
        Banner {
            view,
            title,
            detail,
            resume,
            abort,
        }
    }
}

type Header = (Retained<NSStackView>, Retained<NSTextField>, [Retained<NSButton>; 3]);

/// The branch's popup across the panel's width.
fn branch_row(mtm: MainThreadMarker) -> (Retained<NSStackView>, Retained<NSPopUpButton>) {
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(120.0, 24.0)),
        false,
    );
    ui_size::follow_control_size(&popup, NSControlSize::Small);
    ui_size::set_font(&popup, &term_ui::kit::style::small_system());
    term_ui::kit::set_label(&popup, Some(t("Branch")));
    popup.setToolTip(Some(&ns(t("Branch wechseln oder anlegen"))));
    popup.setContentCompressionResistancePriority_forOrientation(200.0, NSLayoutConstraintOrientation::Horizontal);
    popup.setContentHuggingPriority_forOrientation(200.0, NSLayoutConstraintOrientation::Horizontal);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.addView_inGravity(&popup, NSStackViewGravity::Leading);
    (row, popup)
}

fn header(mtm: MainThreadMarker, panel: &GitPanel) -> Header {
    let distance = label(mtm, "");
    ui_size::set_font(
        &distance,
        &NSFont::monospacedDigitSystemFontOfSize_weight(
            NSFont::smallSystemFontSize(),
            term_render::constant!(objc2_app_kit::NSFontWeightRegular),
        ),
    );
    distance.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let button = |name: &str, title: &str, action: Sel| header_button(mtm, name, title, Some(panel), action);
    let fetch = button("arrow.triangle.2.circlepath", t("Fetch"), sel!(termGitFetch:));
    let pull = button("arrow.down.circle", t("Pull"), sel!(termGitPull:));
    let push = button("arrow.up.circle", t("Push"), sel!(termGitPush:));
    let row = header_row(mtm);
    for view in [&*distance as &NSView, &fetch, &pull, &push] {
        row.addArrangedSubview(view);
    }
    let before = distance.clone();
    ui_size::keep(&*row, Aspect::Layout, move |row| {
        row.setCustomSpacing_afterView(ui_size::scaled(term_ui::kit::style::space::S), &before);
    });
    (row, distance, [fetch, pull, push])
}

fn progress_row(
    mtm: MainThreadMarker,
    panel: &GitPanel,
) -> (
    Retained<NSStackView>,
    Retained<NSProgressIndicator>,
    Retained<NSTextField>,
    Retained<NSButton>,
) {
    let progress = NSProgressIndicator::new(mtm);
    progress.setStyle(NSProgressIndicatorStyle::Bar);
    progress.setControlSize(NSControlSize::Small);
    progress.setIndeterminate(true);
    progress.setMinValue(0.0);
    progress.setMaxValue(100.0);
    progress.setTranslatesAutoresizingMaskIntoConstraints(false);
    let width = progress.widthAnchor().constraintEqualToConstant(56.0);
    ui_size::follow_length(&width, 56.0);
    width.setActive(true);
    let text = label(mtm, "");
    ui_size::set_font(&text, &term_ui::kit::style::small_system());
    text.setTextColor(Some(&NSColor::secondaryLabelColor()));
    text.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    text.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    let cancel = ui_size::symbol_button(
        mtm,
        "xmark.circle.fill",
        t("Abbrechen"),
        term_ui::kit::style::symbol::SMALL,
        Some(panel),
        sel!(termGitCancel:),
    );
    cancel.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    ui_size::follow_spacing(&row, term_ui::kit::style::space::S);
    row.addView_inGravity(&progress, NSStackViewGravity::Leading);
    row.addView_inGravity(&text, NSStackViewGravity::Leading);
    row.addView_inGravity(&cancel, NSStackViewGravity::Trailing);
    (row, progress, text, cancel)
}

/// The note that git runs without the repository's hooks, filters and helpers, with the way to trust the workspace.
fn untrusted_row(mtm: MainThreadMarker, panel: &GitPanel) -> Retained<NSView> {
    let icon = NSImageView::new(mtm);
    ui_size::follow_symbol_image(&icon, "lock.shield", "");
    icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    term_ui::kit::hide_from_voiceover(&icon);
    icon.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    let text = label(mtm, t("Arbeitsbereich nicht vertraut"));
    ui_size::set_font(&text, &term_ui::kit::style::small_system());
    text.setTextColor(Some(&NSColor::secondaryLabelColor()));
    text.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    text.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    let trust = borderless_button(mtm, t("Arbeitsbereich vertrauen"), Some(panel), sel!(termGitTrust:));
    ui_size::set_font(&trust, &term_ui::kit::style::small_system());
    trust.setContentTintColor(Some(&NSColor::controlAccentColor()));
    trust.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    let row = NSView::new(mtm);
    for part in [&*icon as &NSView, &text, &trust] {
        part.setTranslatesAutoresizingMaskIntoConstraints(false);
        row.addSubview(part);
    }
    let gap = text
        .leadingAnchor()
        .constraintEqualToAnchor_constant(&icon.trailingAnchor(), term_ui::kit::style::space::S);
    ui_size::follow_length(&gap, term_ui::kit::style::space::S);
    activate([
        icon.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()),
        icon.centerYAnchor().constraintEqualToAnchor(&text.centerYAnchor()),
        gap,
        text.topAnchor().constraintEqualToAnchor(&row.topAnchor()),
        text.trailingAnchor()
            .constraintLessThanOrEqualToAnchor(&row.trailingAnchor()),
        trust
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&text.leadingAnchor(), -LINK_BEARING),
        trust.topAnchor().constraintEqualToAnchor(&text.bottomAnchor()),
        trust
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor(&row.trailingAnchor()),
        trust.bottomAnchor().constraintEqualToAnchor(&row.bottomAnchor()),
    ]);
    row
}

fn problem_row(
    mtm: MainThreadMarker,
    panel: &GitPanel,
) -> (Retained<NSStackView>, Retained<NSTextField>, Retained<NSButton>) {
    let icon = NSImageView::new(mtm);
    ui_size::follow_symbol_image(&icon, "exclamationmark.triangle.fill", t("Fehler"));
    icon.setContentTintColor(Some(&NSColor::systemOrangeColor()));
    icon.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    let text = NSTextField::wrappingLabelWithString(&ns(""), mtm);
    ui_size::set_font(&text, &term_ui::kit::style::small_system());
    text.setSelectable(true);
    let output = borderless_button(mtm, t("Details …"), Some(panel), sel!(termGitShowOutput:));
    ui_size::set_font(&output, &term_ui::kit::style::small_system());
    output.setContentTintColor(Some(&NSColor::controlAccentColor()));
    let dismiss = ui_size::symbol_button(
        mtm,
        "xmark",
        t("Meldung schließen"),
        10.0,
        Some(panel),
        sel!(termGitDismissProblem:),
    );
    dismiss.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    let texts = NSStackView::new(mtm);
    texts.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    texts.setAlignment(NSLayoutAttribute::Leading);
    ui_size::follow_spacing(&texts, term_ui::kit::style::space::XXS);
    texts.addArrangedSubview(&text);
    texts.addArrangedSubview(&output);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::Top);
    ui_size::follow_spacing(&row, term_ui::kit::style::space::S);
    row.addView_inGravity(&icon, NSStackViewGravity::Leading);
    row.addView_inGravity(&texts, NSStackViewGravity::Leading);
    row.addView_inGravity(&dismiss, NSStackViewGravity::Trailing);
    row.setAccessibilityLabel(Some(&ns(t("Git-Meldung"))));
    (row, text, output)
}

/// The message field with its placeholder on top, in a view of their own: a scroll view draws its clip view over
/// subviews added to it.
fn message_box(
    mtm: MainThreadMarker,
    panel: &GitPanel,
) -> (Retained<NSView>, Retained<CommitTextView>, Retained<NSTextField>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(220.0, MESSAGE_HEIGHT));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setBorderType(NSBorderType::BezelBorder);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    let text = CommitTextView::new(mtm, frame);
    ui_size::keep(&*text, Aspect::Font, |text| {
        text.setFont(Some(&ui_size::system(NSFont::systemFontSize())));
        text.keep_corner_free();
    });
    text.setTextContainerInset(NSSize::new(2.0, 4.0));
    text.setDelegate(Some(ProtocolObject::from_ref(panel)));
    text.setAccessibilityLabel(Some(&ns(t("Commit-Nachricht"))));
    scroll.setDocumentView(Some(&text));
    let container = NSView::new(mtm);
    container.addSubview(&scroll);
    pin_edges(&scroll, &container);
    let height = container.heightAnchor().constraintEqualToConstant(MESSAGE_HEIGHT);
    ui_size::follow_length(&height, MESSAGE_HEIGHT);
    height.setActive(true);
    let placeholder = label(mtm, "");
    placeholder.setTextColor(Some(&NSColor::tertiaryLabelColor()));
    ui_size::set_system_font(&placeholder, NSFont::systemFontSize());
    placeholder.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    placeholder
        .setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    term_ui::kit::hide_from_voiceover(&placeholder);
    container.addSubview(&placeholder);
    placeholder.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        placeholder
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&container.leadingAnchor(), 9.0),
        placeholder
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&container.trailingAnchor(), -6.0),
        placeholder
            .topAnchor()
            .constraintEqualToAnchor_constant(&container.topAnchor(), 5.0),
    ]);
    (container, text, placeholder)
}

fn commit_row(
    mtm: MainThreadMarker,
    panel: &GitPanel,
) -> (Retained<NSStackView>, Retained<NSButton>, Retained<NSButton>) {
    // SAFETY: The panel answers the action with a method that takes the sender; `set_action` checks it again.
    let amend = unsafe { NSButton::checkboxWithTitle_target_action(&ns(t("Letzten Commit ändern")), None, None, mtm) };
    set_action(&amend, panel, sel!(termGitAmend:));
    ui_size::follow_control_size(&amend, NSControlSize::Small);
    ui_size::set_font(&amend, &term_ui::kit::style::small_system());
    amend.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let commit = unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Commit")), None, None, mtm) };
    set_action(&commit, panel, sel!(termGitCommit:));
    commit.setBezelStyle(NSBezelStyle::Push);
    ui_size::follow_control_size(&commit, NSControlSize::Small);
    // AppKit leaves the regular font on a button whose control size shrinks.
    ui_size::set_system_font(&commit, NSFont::systemFontSize());
    commit.setBezelColor(Some(&NSColor::controlAccentColor()));
    commit.setToolTip(Some(&ns(t("Committen (⌘↩)"))));
    let more = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(28.0, 22.0)),
        true,
    );
    ui_size::follow_control_size(&more, NSControlSize::Small);
    more.setTranslatesAutoresizingMaskIntoConstraints(false);
    let width = more.widthAnchor().constraintEqualToConstant(34.0);
    ui_size::follow_length(&width, 34.0);
    width.setActive(true);
    term_ui::kit::set_label(&more, Some(t("Weitere Commit-Aktionen")));
    more.setToolTip(Some(&ns(t("Weitere Commit-Aktionen"))));
    if let Some(menu) = more.menu() {
        menu.addItem(&term_ui::kit::item(mtm, "", None, ""));
        for (title, action) in [
            (t("Committen und pushen"), Some(sel!(termGitCommitPush:))),
            (t("Committen und synchronisieren"), Some(sel!(termGitCommitSync:))),
            ("", None),
            (t("Stash …"), Some(sel!(termGitStash:))),
            (t("Nur Vorgemerktes stashen"), Some(sel!(termGitStashStaged:))),
        ] {
            let Some(action) = action else {
                menu.addItem(&term_ui::kit::separator(mtm));
                continue;
            };
            let entry = term_ui::kit::item(mtm, title, Some(action), "");
            term_ui::kit::set_target(&entry, panel);
            menu.addItem(&entry);
        }
    }
    commit.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let buttons = NSStackView::new(mtm);
    buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    ui_size::follow_spacing(&buttons, term_ui::kit::style::space::XS);
    buttons.addArrangedSubview(&commit);
    buttons.addArrangedSubview(&more);
    // Two lines, as the sidebar at its narrowest has no room for the checkbox's whole title beside the buttons.
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    row.setAlignment(NSLayoutAttribute::Leading);
    ui_size::follow_spacing(&row, term_ui::kit::style::space::M);
    row.addArrangedSubview(&amend);
    row.addArrangedSubview(&buttons);
    buttons
        .widthAnchor()
        .constraintEqualToAnchor(&row.widthAnchor())
        .setActive(true);
    (row, amend, commit)
}

fn outline(mtm: MainThreadMarker, panel: &GitPanel) -> (Retained<NSScrollView>, Retained<GitOutlineView>) {
    let view = GitOutlineView::new(mtm, term_ui::kit::panel::LIST_FRAME);
    let scroll = term_ui::kit::panel::source_list_outline(mtm, &view, "git", t("Git-Änderungen"));
    ui_size::follow_row_heights(&view);
    // Without a gap between rows the graph's lines run on from one commit to the next.
    view.setIntercellSpacing(NSSize::new(3.0, 0.0));
    view.setAllowsMultipleSelection(true);
    // SAFETY: The outline view holds its data source and delegate weakly; the panel registry keeps the panel alive.
    unsafe {
        view.setDataSource(Some(ProtocolObject::from_ref(panel)));
        view.setDelegate(Some(ProtocolObject::from_ref(panel)));
    }
    set_action(&view, panel, sel!(termGitRowClicked:));
    assert!(objc2_foundation::NSObjectProtocol::respondsToSelector(
        panel,
        sel!(termGitRowDoubleClicked:)
    ));
    // SAFETY: The panel answers the double action with a method that takes the sender, checked above.
    unsafe { view.setDoubleAction(Some(sel!(termGitRowDoubleClicked:))) };
    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(panel)));
    // SAFETY: The view keeps the menu; its delegate, the panel, outlives the view through the registry.
    unsafe { view.setMenu(Some(&menu)) };
    (scroll, view)
}

/// A symbol, a title, a line of explanation and maybe a button that sends `action` to the panel.
pub fn empty_state(
    mtm: MainThreadMarker,
    panel: &objc2::runtime::AnyObject,
    symbol_name: &str,
    title: &str,
    detail: &str,
    button: Option<(&str, Sel)>,
) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::CenterX);
    ui_size::follow_spacing(&stack, term_ui::kit::style::space::M);
    stack.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    });
    if let Some(image) = symbol(symbol_name, title) {
        let view = NSImageView::imageViewWithImage(&image, mtm);
        ui_size::keep(&*view, Aspect::Symbol, |view| {
            view.setSymbolConfiguration(Some(
                &objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                    ui_size::scaled(28.0),
                    term_render::constant!(objc2_app_kit::NSFontWeightLight),
                ),
            ));
        });
        view.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
        stack.addArrangedSubview(&view);
    }
    let heading = label(mtm, title);
    ui_size::set_font(&heading, &NSFont::boldSystemFontOfSize(term_ui::kit::style::HEADING));
    heading.setAlignment(NSTextAlignment::Center);
    stack.addArrangedSubview(&heading);
    let explanation = NSTextField::wrappingLabelWithString(&ns(detail), mtm);
    ui_size::set_system_font(&explanation, term_ui::kit::style::BODY);
    explanation.setTextColor(Some(&NSColor::secondaryLabelColor()));
    explanation.setAlignment(NSTextAlignment::Center);
    explanation.setPreferredMaxLayoutWidth(180.0);
    stack.addArrangedSubview(&explanation);
    if let Some((title, action)) = button {
        // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
        let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
        set_action(&button, panel, action);
        button.setBezelStyle(NSBezelStyle::Push);
        ui_size::follow_control_size(&button, NSControlSize::Regular);
        ui_size::set_system_font(&button, NSFont::systemFontSize());
        stack.addArrangedSubview(&button);
    }
    stack
}
