//! The command palette (F102) and the tab switcher (F57) on it: a search field over the window's top with the entries
//! it finds below, each with its shortcut or where it is. Typing filters, the arrows move, Return or a click runs the
//! entry, Escape or a click elsewhere closes. One palette is open at a time, in the window it was opened in.

mod model;

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSBox, NSBoxType, NSColor, NSControl, NSControlTextEditingDelegate, NSEvent, NSFocusRingType,
    NSFont, NSImageView, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLineBreakMode, NSResponder, NSStackView,
    NSTextField, NSTextFieldDelegate, NSTextView, NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSEdgeInsets, NSNotification, NSObject, NSObjectProtocol, NSRect};
use splat_i18n::t;

pub(crate) use model::first_shown;
pub use model::{glyphs, rank};

use crate::kit::{activate, guarded, label, main_soon, ns};

/// How many entries show at once.
const SHOWN: usize = 12;
/// How wide the palette is where the window has room.
const WIDTH: f64 = 560.0;
const ROW_HEIGHT: f64 = 28.0;
const RADIUS: f64 = 12.0;

/// What the palette lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the entry shows.
    pub title: String,
    /// Words that find the entry too without showing, such as its name in the settings file.
    pub hidden: String,
    /// What it shows on the right, such as its shortcut, if anything.
    pub shortcut: String,
    /// What the palette hands back when the entry is chosen.
    pub tag: usize,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermPaletteDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSControlTextEditingDelegate for Delegate {
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            guarded(false, || command_key(command))
        }
    }

    unsafe impl NSTextFieldDelegate for Delegate {}

    impl Delegate {
        #[unsafe(method(controlTextDidChange:))]
        fn text_changed(&self, _notification: &NSNotification) {
            guarded((), filter);
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn ended(&self, _notification: &NSNotification) {
            guarded((), close);
        }
    }
);

define_class!(
    // SAFETY: NSView has no subclassing requirements and `RowView` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermPaletteRow"]
    #[ivars = Cell<usize>]
    struct RowView;

    unsafe impl NSObjectProtocol for RowView {}

    impl RowView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            guarded((), || choose_row(self.ivars().get()));
        }

        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_press(&self) -> bool {
            guarded(false, || {
                choose_row(self.ivars().get());
                true
            })
        }
    }
);

impl RowView {
    fn new(mtm: MainThreadMarker, row: usize) -> Retained<RowView> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(row));
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }
}

/// A row of the list and its labels.
struct Row {
    view: Retained<RowView>,
    title: Retained<NSTextField>,
    shortcut: Retained<NSTextField>,
}

/// The open palette.
struct Palette {
    root: Retained<NSView>,
    field: Retained<NSTextField>,
    rows: Vec<Row>,
    nothing: Retained<NSTextField>,
    entries: Vec<Entry>,
    found: Vec<usize>,
    selected: usize,
    first: usize,
    chosen: fn(usize),
    /// What had the keyboard before, which gets it back.
    previous: Option<Weak<NSResponder>>,
    _delegate: Retained<Delegate>,
}

thread_local! {
    static OPEN: RefCell<Option<Palette>> = const { RefCell::new(None) };
}

fn with_open<R>(f: impl FnOnce(&mut Palette) -> R) -> Option<R> {
    OPEN.with(|open| open.borrow_mut().as_mut().map(f))
}

fn row(mtm: MainThreadMarker, index: usize) -> Row {
    let view = RowView::new(mtm, index);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(6.0);
    }
    view.setAccessibilityElement(true);
    view.setAccessibilityRole(Some(term_render::constant!(objc2_app_kit::NSAccessibilityButtonRole)));
    let title = label(mtm, "");
    title.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    title.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let shortcut = label(mtm, "");
    shortcut.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    shortcut.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let line = NSStackView::new(mtm);
    line.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    line.setAlignment(NSLayoutAttribute::CenterY);
    line.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 8.0,
        bottom: 0.0,
        right: 8.0,
    });
    let spacer = NSView::new(mtm);
    spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    for part in [&*title, &*spacer, &*shortcut] {
        line.addArrangedSubview(part);
    }
    view.addSubview(&line);
    crate::kit::pin_edges(&line, &view);
    activate([view.heightAnchor().constraintEqualToConstant(ROW_HEIGHT)]);
    Row { view, title, shortcut }
}

/// The search line at the palette's top: a magnifying glass and a large borderless field, as Spotlight has.
fn search_line(mtm: MainThreadMarker, placeholder: &str) -> (Retained<NSView>, Retained<NSTextField>) {
    let field = NSTextField::new(mtm);
    field.setBordered(false);
    field.setBezeled(false);
    field.setDrawsBackground(false);
    field.setFocusRingType(NSFocusRingType::None);
    field.setFont(Some(&NSFont::systemFontOfSize(20.0)));
    field.setPlaceholderString(Some(&ns(placeholder)));
    crate::kit::set_label(&field, Some(placeholder));
    if let Some(cell) = field.cell() {
        cell.setScrollable(true);
        cell.setWraps(false);
    }
    let glass =
        crate::kit::symbol_at("magnifyingglass", "", 18.0).map(|image| NSImageView::imageViewWithImage(&image, mtm));
    let line = NSStackView::new(mtm);
    line.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    line.setAlignment(NSLayoutAttribute::CenterY);
    line.setSpacing(crate::kit::space::M);
    line.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 14.0,
        bottom: 12.0,
        right: 14.0,
    });
    if let Some(glass) = glass {
        glass.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        line.addArrangedSubview(&glass);
    }
    line.addArrangedSubview(&field);
    (line.into_super(), field)
}

/// The palette's background, the search line and the list in a column, with the shadow of a panel over the window.
fn assemble(mtm: MainThreadMarker, search: &NSView, rows: &[Row], nothing: &NSTextField) -> Retained<NSView> {
    let background = NSVisualEffectView::new(mtm);
    background.setMaterial(NSVisualEffectMaterial::Menu);
    background.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    background.setState(NSVisualEffectState::Active);
    background.setWantsLayer(true);
    let increase_contrast = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
    if let Some(layer) = background.layer() {
        layer.setCornerRadius(RADIUS);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(if increase_contrast { 1.0 } else { 0.5 });
        let border = if increase_contrast {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::separatorColor()
        };
        layer.setBorderColor(Some(&border.CGColor()));
    }
    let divider = NSBox::new(mtm);
    divider.setBoxType(NSBoxType::Separator);
    let list = NSStackView::new(mtm);
    list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    list.setSpacing(crate::kit::space::NONE);
    list.setEdgeInsets(NSEdgeInsets {
        top: 6.0,
        left: 6.0,
        bottom: 6.0,
        right: 6.0,
    });
    for row in rows {
        list.addArrangedSubview(&row.view);
        crate::kit::pin_sides(&row.view, &list, 6.0);
    }
    list.addArrangedSubview(nothing);
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setSpacing(crate::kit::space::NONE);
    for part in [search, &divider, &list] {
        column.addArrangedSubview(part);
        crate::kit::pin_sides(part, &column, 0.0);
    }
    background.addSubview(&column);
    crate::kit::pin_edges(&column, &background);
    let shadowed = NSView::new(mtm);
    shadowed.setWantsLayer(true);
    if let Some(layer) = shadowed.layer() {
        layer.setShadowOpacity(0.3);
        layer.setShadowRadius(18.0);
        layer.setShadowOffset(objc2_foundation::NSSize::new(0.0, -8.0));
    }
    shadowed.addSubview(&background);
    crate::kit::pin_edges(&background, &shadowed);
    shadowed
}

/// Opens the palette over `window` with `entries`; `chosen` runs with the tag of the entry chosen, once the palette
/// closed and the keyboard is back where it was. A palette already open closes instead.
pub fn open(window: &NSWindow, entries: Vec<Entry>, chosen: fn(usize)) {
    open_with(window, t("Aktion suchen"), entries, chosen);
}

/// Opens the palette as `open` does, with `placeholder` in its empty field.
pub fn open_with(window: &NSWindow, placeholder: &str, entries: Vec<Entry>, chosen: fn(usize)) {
    if is_open() {
        return close();
    }
    let Some(content) = window.contentView() else { return };
    let mtm = window.mtm();
    let delegate: Retained<Delegate> = {
        let this = Delegate::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    };
    let (search, field) = search_line(mtm, placeholder);
    // SAFETY: The field holds its delegate weakly; the palette keeps the delegate as long as the field.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    let rows: Vec<Row> = (0..SHOWN).map(|index| row(mtm, index)).collect();
    let nothing = label(mtm, t("Keine Treffer"));
    nothing.setTextColor(Some(&NSColor::secondaryLabelColor()));
    nothing.setFont(Some(&NSFont::systemFontOfSize(15.0)));
    nothing.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    activate([nothing.heightAnchor().constraintEqualToConstant(ROW_HEIGHT * 2.0)]);
    let root = assemble(mtm, &search, &rows, &nothing);
    content.addSubview(&root);
    root.setTranslatesAutoresizingMaskIntoConstraints(false);
    let wide = root.widthAnchor().constraintEqualToConstant(WIDTH);
    wide.setPriority(500.0);
    activate([
        root.centerXAnchor().constraintEqualToAnchor(&content.centerXAnchor()),
        root.topAnchor()
            .constraintEqualToAnchor_constant(&content.safeAreaLayoutGuide().topAnchor(), 24.0),
        root.widthAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&content.widthAnchor(), -32.0),
        wide,
    ]);
    let previous = window.firstResponder().map(|responder| Weak::from_retained(&responder));
    let found = (0..entries.len()).collect();
    OPEN.with(|open| {
        *open.borrow_mut() = Some(Palette {
            root,
            field: field.clone(),
            rows,
            nothing,
            entries,
            found,
            selected: 0,
            first: 0,
            chosen,
            previous,
            _delegate: delegate,
        });
    });
    show();
    window.makeFirstResponder(Some(&field));
}

/// Whether the palette is open.
pub fn is_open() -> bool {
    OPEN.with(|open| open.borrow().is_some())
}

/// Closes the palette and gives the keyboard back.
pub fn close() {
    let Some(palette) = OPEN.with(|open| open.borrow_mut().take()) else {
        return;
    };
    let window = palette.root.window();
    palette.root.removeFromSuperview();
    if let (Some(window), Some(previous)) = (window, palette.previous.and_then(|weak| weak.load())) {
        window.makeFirstResponder(Some(&previous));
    }
}

/// Runs the entry `found` shows at `index`.
fn choose(index: usize) {
    let Some((tag, chosen)) = with_open(|palette| {
        let entry = palette.found.get(index)?;
        Some((palette.entries[*entry].tag, palette.chosen))
    })
    .flatten() else {
        return;
    };
    close();
    main_soon(move || chosen(tag));
}

/// Runs the entry in row `row` of the list.
fn choose_row(row: usize) {
    if let Some(index) = with_open(|palette| palette.first + row) {
        choose(index);
    }
}

/// Arrows move through the list, Return runs, Escape closes; whether the command was handled.
fn command_key(command: Sel) -> bool {
    let step: isize = if command == sel!(moveUp:) {
        -1
    } else if command == sel!(moveDown:) {
        1
    } else if command == sel!(insertNewline:) {
        if let Some(selected) = with_open(|palette| palette.selected) {
            choose(selected);
        }
        return true;
    } else if command == sel!(cancelOperation:) {
        close();
        return true;
    } else {
        return false;
    };
    with_open(|palette| {
        let last = palette.found.len().saturating_sub(1);
        palette.selected = palette.selected.saturating_add_signed(step).min(last);
    });
    show();
    true
}

/// Finds anew what the field holds.
fn filter() {
    with_open(|palette| {
        let query = palette.field.stringValue().to_string();
        let entries: Vec<(&str, &str)> = palette
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.hidden.as_str()))
            .collect();
        palette.found = model::rank(&query, &entries);
        palette.selected = 0;
        palette.first = 0;
    });
    show();
}

/// Shows the rows around the selected entry.
fn show() {
    with_open(|palette| {
        palette.first = model::first_shown(palette.first, palette.selected, SHOWN, palette.found.len());
        for (at, row) in palette.rows.iter().enumerate() {
            let index = palette.first + at;
            let Some(entry) = palette.found.get(index).map(|entry| &palette.entries[*entry]) else {
                row.view.setHidden(true);
                continue;
            };
            row.view.setHidden(false);
            row.title.setStringValue(&ns(&entry.title));
            let spoken = [entry.title.as_str(), entry.shortcut.as_str()]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            row.view.setAccessibilityLabel(Some(&ns(&spoken)));
            row.shortcut.setStringValue(&ns(&entry.shortcut));
            let selected = index == palette.selected;
            row.view.setAccessibilitySelected(selected);
            let (text, secondary) = if selected {
                (
                    NSColor::alternateSelectedControlTextColor(),
                    NSColor::alternateSelectedControlTextColor(),
                )
            } else {
                (NSColor::labelColor(), NSColor::secondaryLabelColor())
            };
            row.title.setTextColor(Some(&text));
            row.shortcut.setTextColor(Some(&secondary));
            if let Some(layer) = row.view.layer() {
                let background = if selected {
                    NSColor::selectedContentBackgroundColor()
                } else {
                    NSColor::clearColor()
                };
                layer.setBackgroundColor(Some(&background.CGColor()));
            }
        }
        palette.nothing.setHidden(!palette.found.is_empty());
    });
}

/// Types `query` into the open palette, as the keyboard would, and runs the entry it selects with `run`; the titles
/// it found, the best first.
#[cfg(feature = "selftest")]
pub fn drive(query: &str, run: bool) -> Vec<String> {
    with_open(|palette| palette.field.setStringValue(&ns(query)));
    filter();
    let titles = with_open(|palette| {
        palette
            .found
            .iter()
            .map(|index| palette.entries[*index].title.clone())
            .collect()
    })
    .unwrap_or_default();
    if run && let Some(selected) = with_open(|palette| palette.selected) {
        choose(selected);
    }
    titles
}
