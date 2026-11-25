//! A workspace's row of the sidebar as a table cell view, sized by the interface size of the settings as Dango sizes its
//! sidebar, and tinted as the system tints a source list: icons in the accent colour, white on the emphasized selection.

use std::cell::{OnceCell, RefCell};
use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSBackgroundStyle, NSButton, NSCellImagePosition, NSColor, NSFont, NSFontTextStyle,
    NSFontTextStyleBody, NSFontTextStyleSubheadline, NSImageView, NSLayoutAttribute, NSLayoutConstraintOrientation,
    NSLineBreakMode, NSStackView, NSStackViewGravity, NSTableCellView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSDictionary, NSObjectProtocol, NSRect};
use splat_i18n::{t, tf};
use term_ui::kit::{self, guarded, label, ns, ui_size};

use crate::model::{Connection, Mark, Panel, Remote, Workspace};

/// The point size of the status dot.
const DOT: f64 = 6.0;
/// Where the dot's centre sits from the cell's trailing and top edge: in the middle of the selection's rounded corner.
const CORNER: (f64, f64) = (1.0, 6.0);
/// The pencil's size against the panel switches'.
const PENCIL_SCALE: f64 = 0.8;
/// The room between a row's two lines.
const LINE_GAP: f64 = 1.0;
/// How much wider and taller than its symbol a panel switch is, so that it is easy to hit.
const SWITCH_PADDING: f64 = 9.0;
/// The width of a digit against its font's point size.
const DIGIT_WIDTH: f64 = 0.62;

/// How a row looks at the interface size.
pub(crate) struct Size {
    heading: Retained<NSFont>,
    detail: Retained<NSFont>,
    /// The room above and below the two lines.
    padding: f64,
    /// The point size of the panel switches' symbols.
    switch: f64,
    /// The digits of the largest number of changed files among the rows, which every row keeps room for so that the
    /// switches of all rows line up.
    count_digits: usize,
}

impl Size {
    /// The size at the interface size, with room for a number of changed files of `count_digits` digits; at the
    /// default it is the medium of macOS's three sidebar sizes.
    pub(crate) fn of(count_digits: usize) -> Size {
        // SAFETY: AppKit sets its exported text style constants when it loads and never changes them.
        let (heading, detail) = unsafe { (NSFontTextStyleBody, NSFontTextStyleSubheadline) };
        Size {
            heading: ui_size::font_like(&text_style(heading)),
            detail: ui_size::font_like(&text_style(detail)),
            padding: ui_size::scaled(6.0),
            switch: ui_size::scaled(13.0),
            count_digits,
        }
    }

    /// The width of the Git switch with the number of changed files beside its symbol.
    #[expect(clippy::cast_precision_loss, reason = "a few digits")]
    fn counting_switch(&self) -> f64 {
        let digits = (self.count_digits as f64 * self.detail.pointSize() * DIGIT_WIDTH).ceil();
        self.switch + ui_size::scaled(SWITCH_PADDING) - 1.0 + digits
    }

    /// The height of a row, the same for every row whether it has a second line or not.
    pub(crate) fn row_height(&self) -> f64 {
        (line_height(&self.heading) + ui_size::scaled(LINE_GAP) + line_height(&self.detail) + 2.0 * self.padding).ceil()
    }
}

fn text_style(style: &NSFontTextStyle) -> Retained<NSFont> {
    // SAFETY: An empty options dictionary is a valid argument.
    unsafe { NSFont::preferredFontForTextStyle_options(style, &NSDictionary::new()) }
}

fn line_height(font: &NSFont) -> f64 {
    (font.ascender() - font.descender() + font.leading()).ceil()
}

/// What a cell keeps: its buttons with the colour each shows while its row is not emphasized, and the name.
pub(crate) struct Ivars {
    toggles: RefCell<Vec<(Retained<NSButton>, Retained<NSColor>)>>,
    name: OnceCell<Retained<NSTextField>>,
}

define_class!(
    // SAFETY: NSTableCellView has no subclassing requirements and `Cell` does not implement Drop.
    #[unsafe(super(NSTableCellView, NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSidebarCell"]
    #[ivars = Ivars]
    pub(crate) struct Cell;

    unsafe impl NSObjectProtocol for Cell {}

    impl Cell {
        #[unsafe(method(setBackgroundStyle:))]
        fn set_background_style(&self, style: NSBackgroundStyle) {
            guarded((), || {
                // SAFETY: NSTableCellView's own setter, with the style AppKit passed.
                let _: () = unsafe { msg_send![super(self), setBackgroundStyle: style] };
                self.tint(style);
            });
        }
    }
);

impl Cell {
    /// The cell that shows `workspace` at `size`.
    /// With `toggles`, the row's second line ends in the switches of its panels, which send their actions there, and
    /// with `pencil` as well its name in the pencil that renames it.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        workspace: &Workspace,
        home: &Path,
        size: &Size,
        toggles: Option<&AnyObject>,
        pencil: bool,
    ) -> Retained<Cell> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            toggles: RefCell::new(Vec::new()),
            name: OnceCell::new(),
        });
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let this: Retained<Cell> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.fill(workspace, home, size, toggles, pencil);
        this
    }

    fn fill(&self, workspace: &Workspace, home: &Path, size: &Size, toggles: Option<&AnyObject>, pencil: bool) {
        let mtm = self.mtm();
        let heading = text(mtm, &workspace.name, &size.heading);
        heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let detail = text(mtm, &workspace.detail(home), &size.detail);
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let first = line(mtm, &heading);
        if let (false, true, Some(target)) = (workspace.fixed, pencil, toggles) {
            first.addView_inGravity(&self.pencil(workspace.id, size, target), NSStackViewGravity::Leading);
            heading.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
        }
        let second = line(mtm, &detail);
        // A row without a second line shows its name alone at the row's middle.
        second.setHidden(detail.stringValue().length() == 0);
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*first as &NSView, &*second]), mtm);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::Width);
        lines.setSpacing(ui_size::scaled(LINE_GAP));
        lines.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.addSubview(&lines);
        let end = match toggles {
            Some(target) => {
                let switches = self.toggles(workspace, size, target);
                switches.setTranslatesAutoresizingMaskIntoConstraints(false);
                self.addSubview(&switches);
                kit::activate([
                    switches
                        .trailingAnchor()
                        .constraintEqualToAnchor_constant(&self.trailingAnchor(), -kit::space::XXS),
                    switches.centerYAnchor().constraintEqualToAnchor(&self.centerYAnchor()),
                    lines
                        .trailingAnchor()
                        .constraintEqualToAnchor_constant(&switches.leadingAnchor(), -kit::space::S),
                ]);
                None
            }
            None => Some(
                lines
                    .trailingAnchor()
                    .constraintEqualToAnchor_constant(&self.trailingAnchor(), -kit::space::XS),
            ),
        };
        kit::activate(end);
        kit::activate([
            lines.leadingAnchor().constraintEqualToAnchor(&self.leadingAnchor()),
            lines.centerYAnchor().constraintEqualToAnchor(&self.centerYAnchor()),
        ]);
        // The dot sits in the row's top trailing corner, at the same height in every row, and takes no room.
        if let Some(mark) = workspace.mark() {
            let dot = dot(mtm, mark);
            dot.setTranslatesAutoresizingMaskIntoConstraints(false);
            self.addSubview(&dot);
            kit::activate([
                dot.centerXAnchor()
                    .constraintEqualToAnchor_constant(&self.trailingAnchor(), -CORNER.0),
                dot.centerYAnchor()
                    .constraintEqualToAnchor_constant(&self.topAnchor(), CORNER.1),
            ]);
        }
        // Not the cell's text field: AppKit dims that one on a source list's emphasized selection to a pale blue.
        let _ = self.ivars().name.set(heading);
        self.setAccessibilityElement(true);
        // SAFETY: AppKit sets its exported constants when it loads and never changes them.
        self.setAccessibilityRole(Some(unsafe { objc2_app_kit::NSAccessibilityGroupRole }));
        self.setAccessibilityLabel(Some(&ns(&workspace.spoken(home))));
    }

    /// The pencil after the name, which turns the name into a field to rename the workspace.
    fn pencil(&self, id: u64, size: &Size, target: &AnyObject) -> Retained<NSButton> {
        let title = t("Arbeitsbereich umbenennen");
        let pencil = kit::symbol_button(
            self.mtm(),
            "pencil",
            title,
            size.switch * PENCIL_SCALE,
            Some(target),
            sel!(termSidebarEditName:),
        );
        pencil.setTag(isize::try_from(id).unwrap_or(isize::MAX));
        pencil.setToolTip(Some(&ns(title)));
        let tint = NSColor::tertiaryLabelColor();
        pencil.setContentTintColor(Some(&tint));
        self.ivars().toggles.borrow_mut().push((pencil.clone(), tint));
        pencil
    }

    /// The switches of the Git panel and the file list, in that order, which show in the accent colour while their
    /// panel is open; the Git switch with the number of changed files before its symbol, and only for a folder in a
    /// repository.
    fn toggles(&self, workspace: &Workspace, size: &Size, target: &AnyObject) -> Retained<NSStackView> {
        let mtm = self.mtm();
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setSpacing(if size.count_digits > 0 {
            kit::space::XXS
        } else {
            kit::space::NONE
        });
        let side = size.switch + ui_size::scaled(SWITCH_PADDING);
        if let Some(remote) = &workspace.remote {
            let button = self.connection_switch(workspace.id, remote, size, target);
            kit::activate([
                button.widthAnchor().constraintEqualToConstant(side),
                button.heightAnchor().constraintEqualToConstant(side),
            ]);
            row.addArrangedSubview(&button);
            return row;
        }
        let shown = [Panel::Git, Panel::Files]
            .into_iter()
            .filter(|panel| *panel != Panel::Git || workspace.branch.is_some());
        for panel in shown {
            let open = workspace.panels.contains(&panel);
            let (symbol, label, action) = match panel {
                Panel::Files => ("folder", t("Dateien"), sel!(termSidebarToggleFiles:)),
                Panel::Git => ("arrow.triangle.branch", t("Git"), sel!(termSidebarToggleGit:)),
            };
            let button = kit::symbol_button(mtm, symbol, label, size.switch, Some(target), action);
            button.setTag(isize::try_from(workspace.id).unwrap_or(isize::MAX));
            let tint = if open {
                NSColor::controlAccentColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            button.setContentTintColor(Some(&tint));
            let counts = panel == Panel::Git && size.count_digits > 0;
            if counts {
                let changes = workspace.changes;
                button.setTitle(&ns(&if changes > 0 {
                    changes.to_string()
                } else {
                    String::new()
                }));
                button.setImagePosition(NSCellImagePosition::ImageTrailing);
                button.setAlignment(NSTextAlignment::Right);
                button.setFont(Some(&size.detail));
            }
            let spoken = match (panel, workspace.changes) {
                (Panel::Git, changes @ 1..) => tf("{}, geänderte Dateien: {}", &[label, &changes.to_string()]),
                _ => label.to_string(),
            };
            let state = if open { t("geöffnet") } else { t("geschlossen") };
            button.setAccessibilityLabel(Some(&ns(&tf("{}, {}", &[&spoken, state]))));
            button.setTranslatesAutoresizingMaskIntoConstraints(false);
            let width = if counts {
                button
                    .widthAnchor()
                    .constraintGreaterThanOrEqualToConstant(size.counting_switch())
            } else {
                button.widthAnchor().constraintEqualToConstant(side)
            };
            kit::activate([width, button.heightAnchor().constraintEqualToConstant(side)]);
            self.ivars().toggles.borrow_mut().push((button.clone(), tint));
            row.addArrangedSubview(&button);
        }
        row
    }

    /// The switch of a remote workspace's connection in place of the panels', its colour the connection's state; a
    /// click opens the connection's menu (E25).
    fn connection_switch(&self, id: u64, remote: &Remote, size: &Size, target: &AnyObject) -> Retained<NSButton> {
        let label = tf("Verbindung zu {}", &[&remote.account]);
        let button = kit::symbol_button(
            self.mtm(),
            "network",
            &label,
            size.switch,
            Some(target),
            sel!(termSidebarRemote:),
        );
        button.setTag(isize::try_from(id).unwrap_or(isize::MAX));
        let tint = match remote.connection {
            Connection::Connected => NSColor::secondaryLabelColor(),
            Connection::Connecting => NSColor::tertiaryLabelColor(),
            Connection::Disconnected => NSColor::systemOrangeColor(),
            Connection::Failed => NSColor::systemRedColor(),
        };
        button.setContentTintColor(Some(&tint));
        button.setToolTip(Some(&ns(&tf("{}: {}", &[&label, remote.connection.spoken()]))));
        button.setAccessibilityLabel(Some(&ns(&tf("{}, {}", &[&label, remote.connection.spoken()]))));
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.ivars().toggles.borrow_mut().push((button.clone(), tint));
        button
    }

    /// The field that shows the workspace's name.
    pub(crate) fn name(&self) -> Option<Retained<NSTextField>> {
        self.ivars().name.get().cloned()
    }

    /// Whether the row shows the pencil that renames its workspace.
    pub(crate) fn has_pencil(&self) -> bool {
        self.ivars()
            .toggles
            .borrow()
            .iter()
            .any(|(button, _)| button.action() == Some(sel!(termSidebarEditName:)))
    }

    /// The buttons take the colour of the selection's text on the emphasized selection, else their own.
    fn tint(&self, style: NSBackgroundStyle) {
        let emphasized = style == NSBackgroundStyle::Emphasized;
        for (button, tint) in self.ivars().toggles.borrow().iter() {
            let white = NSColor::alternateSelectedControlTextColor();
            button.setContentTintColor(Some(if emphasized { &white } else { tint }));
        }
    }
}

fn text(mtm: MainThreadMarker, value: &str, font: &NSFont) -> Retained<NSTextField> {
    let field = label(mtm, value);
    field.setFont(Some(font));
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    field.setAllowsExpansionToolTips(true);
    field.setAccessibilityElement(false);
    field
}

/// A line of the row: `text` at its leading edge, taking the room the views at its trailing edge leave.
fn line(mtm: MainThreadMarker, text: &NSTextField) -> Retained<NSStackView> {
    let line = NSStackView::new(mtm);
    line.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    line.setSpacing(kit::space::S);
    // The stack must not hug its views, so that its trailing views keep to the row's trailing edge.
    line.setHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    text.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    line.addView_inGravity(text, NSStackViewGravity::Leading);
    line
}

/// The colour of `mark`'s dot, Dango's: green while an agent works, red while one waits for the user.
pub fn mark_color(mark: Mark) -> Retained<NSColor> {
    match mark {
        Mark::Works => NSColor::systemGreenColor(),
        Mark::Waits => NSColor::systemRedColor(),
    }
}

/// The dot for `mark`.
fn dot(mtm: MainThreadMarker, mark: Mark) -> Retained<NSImageView> {
    let dot = NSImageView::new(mtm);
    dot.setImage(kit::symbol_at("circle.fill", "", ui_size::scaled(DOT)).as_deref());
    dot.setContentTintColor(Some(&mark_color(mark)));
    kit::hide_from_voiceover(&dot);
    dot
}
