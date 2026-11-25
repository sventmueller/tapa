//! The format bar over a Markdown document (E195): the document's name, then splat's format buttons in its three
//! groups, and the preview's switch. Each Markdown editor tab has its own, above its text.

use std::cell::RefCell;

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSColor, NSControl, NSFont, NSLineBreakMode, NSMenu, NSMenuItem, NSPopUpButton,
    NSStackView, NSStackViewGravity, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSObjectProtocol, NSRect};
use splat_i18n::t;
use splat_markdown::doc::AlertKind;
use term_ui::kit::style::{self, space};
use term_ui::kit::ui_size;

use super::Command;
use super::format::{Choice, languages};
use super::insert::Diagram;
use crate::EditorTextView;
use crate::util::{activate, guarded, ns, pin_inset, separator, set_target, tagged};

#[cfg(feature = "selftest")]
mod selftest_probe;

/// The bar's height at the default interface size, the AI tab's head's.
const HEIGHT: f64 = 36.0;
/// The room between two buttons.
const SPACING: f64 = space::M;

/// A control of the bar: a button that runs a command, or one that opens a menu of them.
#[derive(Clone, Copy)]
enum Item {
    Button(Command, &'static str),
    Menu(Menu, &'static str),
}

/// The menus of the bar, as splat's toolbar has them.
#[derive(Clone, Copy)]
enum Menu {
    Script,
    Heading,
    List,
    CodeBlock,
    Alert,
    Diagram,
    More,
}

impl Menu {
    fn title(self) -> &'static str {
        match self {
            Menu::Script => t("Hoch- und Tiefstellen"),
            Menu::Heading => t("Überschrift"),
            Menu::List => t("Liste"),
            Menu::CodeBlock => t("Codeblock"),
            Menu::Alert => t("Hinweisbox"),
            Menu::Diagram => t("Diagramm"),
            Menu::More => t("Weitere Elemente"),
        }
    }

    /// Its entries, by title and tag; a separator for `None`.
    fn entries(self) -> Vec<Option<(String, isize)>> {
        let commands = |commands: &[Command]| -> Vec<Option<(String, isize)>> {
            commands
                .iter()
                .map(|command| Some((command.title().to_string(), *command as isize)))
                .collect()
        };
        use Command::*;
        match self {
            Menu::Script => {
                let mut entries = commands(&[Superscript, Subscript]);
                entries.push(None);
                entries.extend(commands(&[RemoveFormatting]));
                entries
            }
            Menu::Heading => commands(&[Paragraph, Heading1, Heading2, Heading3, Heading4, Heading5, Heading6]),
            Menu::List => commands(&[BulletList, NumberedList, TaskList]),
            Menu::CodeBlock => {
                let mut entries = commands(&[Code]);
                entries.push(None);
                for (n, (_, name)) in languages().enumerate() {
                    entries.push(Some((name.to_string(), Choice::CodeBlock(n).tag())));
                    if n == 0 {
                        entries.push(None);
                    }
                }
                entries
            }
            Menu::Alert => AlertKind::ALL
                .iter()
                .map(|kind| Some((alert_title(*kind).to_string(), Choice::Alert(*kind).tag())))
                .collect(),
            Menu::Diagram => Diagram::ALL
                .iter()
                .map(|diagram| Some((diagram.title().to_string(), Choice::Diagram(*diagram).tag())))
                .collect(),
            Menu::More => commands(&[Toc, Footnote, Rule]),
        }
    }
}

fn alert_title(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Note => t("Hinweis"),
        AlertKind::Tip => t("Tipp"),
        AlertKind::Important => t("Wichtig"),
        AlertKind::Warning => t("Warnung"),
        AlertKind::Caution => t("Vorsicht"),
    }
}

/// The groups of splat's toolbar: characters, paragraphs, and what goes in.
const GROUPS: [&[Item]; 3] = [
    &[
        Item::Button(Command::Bold, "bold"),
        Item::Button(Command::Italic, "italic"),
        Item::Button(Command::Underline, "underline"),
        Item::Button(Command::Strikethrough, "strikethrough"),
        Item::Button(Command::Highlight, "highlighter"),
        Item::Menu(Menu::Script, "textformat.superscript"),
    ],
    &[
        Item::Menu(Menu::Heading, "h.square"),
        Item::Menu(Menu::List, "list.bullet"),
        Item::Button(Command::Quote, "text.quote"),
        Item::Menu(Menu::CodeBlock, "curlybraces"),
        Item::Menu(Menu::Alert, "info.bubble"),
    ],
    &[
        Item::Button(Command::Link, "link"),
        Item::Button(Command::Image, "photo"),
        Item::Button(Command::Table, "tablecells"),
        Item::Button(Command::Formula, "function"),
        Item::Menu(Menu::Diagram, "flowchart"),
        Item::Menu(Menu::More, "ellipsis.circle"),
    ],
];

/// The symbol of the preview's switch.
const PREVIEW: &str = "rectangle.split.2x1";

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `BarTarget` does not implement Drop.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermEditorFormatBarTarget"]
    #[ivars = RefCell<Weak<EditorTextView>>]
    struct BarTarget;

    unsafe impl NSObjectProtocol for BarTarget {}

    impl BarTarget {
        #[unsafe(method(termFormatBar:))]
        fn format(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let tag = sender.and_then(|sender| {
                    sender
                        .downcast_ref::<NSMenuItem>()
                        .map(NSMenuItem::tag)
                        .or_else(|| sender.downcast_ref::<NSControl>().map(NSControl::tag))
                });
                let (Some(tag), Some(view)) = (tag, self.ivars().borrow().load()) else {
                    return;
                };
                super::perform(&view, tag);
                if tag != Command::Preview as isize
                    && let Some(window) = view.window()
                {
                    window.makeFirstResponder(Some(&view));
                }
            });
        }
    }
);

impl BarTarget {
    fn new(mtm: MainThreadMarker) -> Retained<BarTarget> {
        let this = BarTarget::alloc(mtm).set_ivars(RefCell::new(Weak::default()));
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The format bar of a Markdown editor.
pub(crate) struct FormatBar {
    pub(crate) view: Retained<NSView>,
    title: Retained<NSTextField>,
    /// The format controls, which a document that cannot be edited greys out.
    controls: Vec<Retained<NSControl>>,
    preview: Retained<NSButton>,
    target: Retained<BarTarget>,
    /// The bar's height, none while it is hidden.
    height: Retained<objc2_app_kit::NSLayoutConstraint>,
}

impl FormatBar {
    pub(crate) fn new(mtm: MainThreadMarker) -> FormatBar {
        let target = BarTarget::new(mtm);
        let title = NSTextField::labelWithString(&ns(""), mtm);
        ui_size::set_font(&title, &NSFont::boldSystemFontOfSize(style::BODY));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        title.setContentCompressionResistancePriority_forOrientation(
            200.0,
            objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
        );
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        ui_size::follow_spacing(&row, SPACING);
        row.addView_inGravity(&title, NSStackViewGravity::Leading);
        // A narrow window drops the last buttons instead of being held at the width of all of them.
        row.setClippingResistancePriority_forOrientation(
            objc2_app_kit::NSLayoutPriorityDefaultLow,
            objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
        );
        let mut controls = Vec::new();
        for (n, item) in GROUPS.iter().flat_map(|group| group.iter()).enumerate() {
            let control = make(mtm, *item, &target);
            row.addView_inGravity(&control, NSStackViewGravity::Trailing);
            row.setVisibilityPriority_forView(
                objc2_app_kit::NSStackViewVisibilityPriorityDetachOnlyIfNecessary - n as f32,
                &control,
            );
            controls.push(control);
        }
        let preview = button(mtm, Command::Preview, PREVIEW, &target);
        row.addView_inGravity(&preview, NSStackViewGravity::Trailing);
        let view = NSView::new(mtm);
        let line = NSBox::new(mtm);
        line.setBoxType(NSBoxType::Separator);
        for part in [&*row as &NSView, &line] {
            view.addSubview(part);
            part.setTranslatesAutoresizingMaskIntoConstraints(false);
        }
        pin_inset(
            &row,
            &view,
            NSEdgeInsets {
                top: 0.0,
                left: space::M,
                bottom: 1.0,
                right: space::XL,
            },
        );
        activate([
            line.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            line.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            line.bottomAnchor().constraintEqualToAnchor(&view.bottomAnchor()),
        ]);
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        let height = view.heightAnchor().constraintEqualToConstant(HEIGHT);
        height.setActive(true);
        height.setConstant(0.0);
        view.setHidden(true);
        objc2_app_kit::NSAccessibility::setAccessibilityLabel(&*view, Some(&ns(t("Format"))));
        FormatBar {
            view,
            title,
            controls,
            preview,
            target,
            height,
        }
    }

    /// Shows the bar for the Markdown editor `editor`, titled `title`, or hides it without one.
    pub(crate) fn show(&self, editor: Option<&EditorTextView>, title: &str) {
        *self.target.ivars().borrow_mut() = editor.map(Weak::from).unwrap_or_default();
        self.view.setHidden(editor.is_none());
        let Some(editor) = editor else {
            self.height.setConstant(0.0);
            return;
        };
        self.height.setConstant(ui_size::scaled(HEIGHT));
        self.title.setStringValue(&ns(title));
        let editable = editor.isEditable();
        for control in &self.controls {
            control.setEnabled(editable);
        }
        let shown = crate::host::installed().is_some_and(|host| host.preview_shown(editor.ivars().serial));
        let color = if shown {
            NSColor::controlAccentColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.preview.setContentTintColor(Some(&color));
        self.preview.setState(if shown {
            objc2_app_kit::NSControlStateValueOn
        } else {
            objc2_app_kit::NSControlStateValueOff
        });
    }
}

fn make(mtm: MainThreadMarker, item: Item, target: &BarTarget) -> Retained<NSControl> {
    match item {
        Item::Button(command, symbol) => Retained::into_super(button(mtm, command, symbol, target)),
        Item::Menu(menu, symbol) => Retained::into_super(Retained::into_super(menu_button(mtm, menu, symbol, target))),
    }
}

fn button(mtm: MainThreadMarker, command: Command, symbol: &str, target: &BarTarget) -> Retained<NSButton> {
    let tip = command.title().trim_end_matches(" …");
    let button = ui_size::symbol_button(
        mtm,
        symbol,
        tip,
        style::symbol::SMALL,
        Some(target),
        sel!(termFormatBar:),
    );
    button.setTag(command as isize);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    crate::util::set_label(&button, Some(tip));
    button
}

/// A borderless pull-down button that shows `symbol` and opens the entries of `menu`.
fn menu_button(mtm: MainThreadMarker, menu: Menu, symbol: &str, target: &BarTarget) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, true);
    popup.setBordered(false);
    let list = NSMenu::new(mtm);
    let head = NSMenuItem::new(mtm);
    head.setTitle(&ns(""));
    list.addItem(&head);
    for entry in menu.entries() {
        let item = match entry {
            Some((title, tag)) => {
                let item = tagged(mtm, &title, sel!(termFormatBar:), "", tag);
                set_target(&item, target);
                item
            }
            None => separator(mtm),
        };
        list.addItem(&item);
    }
    popup.setMenu(Some(&list));
    let (name, title) = (symbol.to_owned(), menu.title());
    ui_size::keep(&*head, ui_size::Aspect::Symbol, move |head| {
        let size = ui_size::scaled(style::symbol::SMALL);
        head.setImage(term_ui::kit::symbol_at(&name, title, size).as_deref());
    });
    popup.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    popup.setToolTip(Some(&ns(title)));
    crate::util::set_label(&popup, Some(title));
    popup
}
