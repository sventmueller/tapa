//! The command palette (F102) and the tab switcher (F57) on it: a search field over the window's top with the entries
//! it finds below, each with its shortcut or where it is. Typing filters, the arrows move, Return or a click runs the
//! entry, Escape or a click elsewhere closes. One palette is open at a time, in the window it was opened in.

mod model;
mod quick_open;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSSearchFieldDelegate;
use objc2_app_kit::{
    NSAccessibility, NSBox, NSBoxType, NSColor, NSControl, NSControlTextEditingDelegate, NSEvent, NSFocusRingType,
    NSFont, NSImage, NSImageView, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLineBreakMode, NSResponder,
    NSStackView, NSTextField, NSTextFieldDelegate, NSTextView, NSUserInterfaceLayoutOrientation, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWorkspace,
};
use objc2_foundation::{NSEdgeInsets, NSNotification, NSObject, NSObjectProtocol, NSRect};
use splat_i18n::t;
use term_base::Cancel;

pub(crate) use model::first_shown;
pub use model::{glyphs, rank};

use crate::kit::{activate, guarded, label, ns};

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

/// A section of a grouped palette, in the order its results should show.
pub struct Group {
    /// The heading above its rows.
    pub title: Option<String>,
    /// The ranked results in this section.
    pub entries: Vec<Entry>,
}

/// Supplies the icon of a result on the main thread, when its row is visible.
pub type ResultIcon = Rc<dyn Fn(usize) -> Option<Retained<NSImage>>>;

struct Grouped {
    cancel: Cancel,
    search: Rc<dyn Fn(String)>,
    query: Option<String>,
    headings: Vec<Option<Rc<str>>>,
    icon: Option<ResultIcon>,
    choose_when_ready: bool,
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

    unsafe impl NSSearchFieldDelegate for Delegate {}

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
    heading: Retained<NSTextField>,
    view: Retained<RowView>,
    title: Retained<NSTextField>,
    shortcut: Retained<NSTextField>,
    icon: Option<Retained<NSImageView>>,
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
    chosen: Rc<dyn Fn(usize)>,
    grouped: Option<Grouped>,
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

fn row(mtm: MainThreadMarker, index: usize, grouped: bool) -> Row {
    let heading = label(mtm, "");
    heading.setFont(Some(&NSFont::boldSystemFontOfSize(11.0)));
    heading.setTextColor(Some(&NSColor::secondaryLabelColor()));
    heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    heading.setHidden(true);
    let view = RowView::new(mtm, index);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(if grouped { 8.0 } else { 6.0 });
    }
    view.setAccessibilityElement(true);
    view.setAccessibilityRole(Some(term_render::constant!(objc2_app_kit::NSAccessibilityButtonRole)));
    let title = label(mtm, "");
    title.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    title.setLineBreakMode(if grouped {
        NSLineBreakMode::ByTruncatingHead
    } else {
        NSLineBreakMode::ByTruncatingTail
    });
    title.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let shortcut = label(mtm, "");
    shortcut.setFont(Some(&NSFont::systemFontOfSize(12.0)));
    shortcut.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let line = NSStackView::new(mtm);
    line.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    line.setAlignment(NSLayoutAttribute::CenterY);
    if grouped {
        line.setSpacing(6.0);
    }
    line.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: if grouped { 12.0 } else { 8.0 },
        bottom: 0.0,
        right: 8.0,
    });
    let spacer = NSView::new(mtm);
    spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    let icon = grouped.then(|| {
        let view = NSImageView::new(mtm);
        view.setImageScaling(objc2_app_kit::NSImageScaling::ScaleNone);
        crate::kit::hide_from_voiceover(&view);
        line.addArrangedSubview(&view);
        activate([
            view.widthAnchor().constraintEqualToConstant(16.0),
            view.heightAnchor().constraintEqualToConstant(16.0),
        ]);
        view
    });
    for part in [&*title, &*spacer, &*shortcut] {
        line.addArrangedSubview(part);
    }
    view.addSubview(&line);
    crate::kit::pin_edges(&line, &view);
    activate([view.heightAnchor().constraintEqualToConstant(if grouped {
        quick_open::ROW_HEIGHT
    } else {
        ROW_HEIGHT
    })]);
    Row {
        heading,
        view,
        title,
        shortcut,
        icon,
    }
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
fn assemble(
    mtm: MainThreadMarker,
    search: &NSView,
    rows: &[Row],
    nothing: &NSTextField,
    grouped: bool,
) -> Retained<NSView> {
    let background = if grouped {
        NSView::new(mtm)
    } else {
        let effect = NSVisualEffectView::new(mtm);
        effect.setMaterial(NSVisualEffectMaterial::Menu);
        effect.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.into_super()
    };
    background.setWantsLayer(true);
    let increase_contrast = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
    if let Some(layer) = background.layer() {
        layer.setCornerRadius(if grouped { 16.0 } else { RADIUS });
        layer.setMasksToBounds(true);
        layer.setBorderWidth(if increase_contrast { 1.0 } else { 0.5 });
        let border = if increase_contrast {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::separatorColor()
        };
        layer.setBorderColor(Some(&border.CGColor()));
        if grouped {
            layer.setBackgroundColor(Some(&NSColor::textBackgroundColor().CGColor()));
        }
    }
    let divider = NSBox::new(mtm);
    divider.setBoxType(NSBoxType::Separator);
    let list = NSStackView::new(mtm);
    list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    list.setSpacing(crate::kit::space::NONE);
    list.setEdgeInsets(NSEdgeInsets {
        top: 6.0,
        left: if grouped { 10.0 } else { 6.0 },
        bottom: 6.0,
        right: 6.0,
    });
    for row in rows {
        list.addArrangedSubview(&row.heading);
        crate::kit::pin_sides(&row.heading, &list, 14.0);
        list.addArrangedSubview(&row.view);
        crate::kit::pin_sides(&row.view, &list, 6.0);
    }
    list.addArrangedSubview(nothing);
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setSpacing(crate::kit::space::NONE);
    let parts: &[&NSView] = if grouped {
        &[search, &list]
    } else {
        &[search, &divider, &list]
    };
    for part in parts {
        column.addArrangedSubview(part);
        crate::kit::pin_sides(part, &column, 0.0);
    }
    if grouped {
        let spacer = NSView::new(mtm);
        spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
        column.addArrangedSubview(&spacer);
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
    open_inner(window, placeholder, entries, Rc::new(chosen), None);
}

/// Opens a grouped palette driven by a background search. Closing the palette cancels its search.
pub fn open_grouped(window: &NSWindow, placeholder: &str, cancel: Cancel, search: Rc<dyn Fn(String)>) {
    if open_inner(
        window,
        placeholder,
        Vec::new(),
        Rc::new(|_| {}),
        Some(Grouped {
            cancel,
            search,
            query: None,
            headings: Vec::new(),
            icon: None,
            choose_when_ready: false,
        }),
    ) {
        filter();
    }
}

fn open_inner(
    window: &NSWindow,
    placeholder: &str,
    entries: Vec<Entry>,
    chosen: Rc<dyn Fn(usize)>,
    grouped: Option<Grouped>,
) -> bool {
    if is_open() {
        close();
        return false;
    }
    let Some(content) = window.contentView() else {
        return false;
    };
    let mtm = window.mtm();
    let delegate: Retained<Delegate> = {
        let this = Delegate::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), init] }
    };
    let quick_open = grouped.is_some();
    let (search, field) = if quick_open {
        quick_open::search_line(mtm, placeholder)
    } else {
        search_line(mtm, placeholder)
    };
    // SAFETY: The field holds its delegate weakly; the palette keeps the delegate as long as the field.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    let rows: Vec<Row> = (0..SHOWN).map(|index| row(mtm, index, quick_open)).collect();
    let nothing = label(mtm, t("Keine Treffer"));
    nothing.setTextColor(Some(&NSColor::secondaryLabelColor()));
    nothing.setFont(Some(&NSFont::systemFontOfSize(15.0)));
    nothing.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    activate([nothing.heightAnchor().constraintEqualToConstant(ROW_HEIGHT * 2.0)]);
    let root = assemble(mtm, &search, &rows, &nothing, quick_open);
    content.addSubview(&root);
    root.setTranslatesAutoresizingMaskIntoConstraints(false);
    let wide = root
        .widthAnchor()
        .constraintEqualToConstant(if quick_open { quick_open::WIDTH } else { WIDTH });
    wide.setPriority(500.0);
    let safe = content.safeAreaLayoutGuide();
    if quick_open {
        let height = root.heightAnchor().constraintEqualToConstant(quick_open::HEIGHT);
        height.setPriority(500.0);
        activate([
            height,
            root.heightAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&safe.heightAnchor(), -20.0),
        ]);
    }
    activate([
        root.centerXAnchor().constraintEqualToAnchor(&content.centerXAnchor()),
        root.topAnchor()
            .constraintEqualToAnchor_constant(&safe.topAnchor(), if quick_open { 8.0 } else { 24.0 }),
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
            grouped,
            previous,
            _delegate: delegate,
        });
    });
    show();
    if quick_open {
        // The search field needs its final bounds before it acquires a field editor.
        window.layoutIfNeeded();
    }
    window.makeFirstResponder(Some(&field));
    true
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
    if let Some(grouped) = palette.grouped {
        grouped.cancel.cancel();
    }
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
        Some((palette.entries[*entry].tag, palette.chosen.clone()))
    })
    .flatten() else {
        return;
    };
    close();
    // The handler is local to AppKit and may own the results it will open.
    term_base::main_thread::after_frame(move || chosen(tag));
}

/// Runs the entry in row `row` of the list.
fn choose_row(row: usize) {
    if let Some(index) = with_open(|palette| palette.first + row) {
        choose(index);
    }
}

fn confirm_selection() {
    let selected = with_open(|palette| {
        if palette.found.is_empty() {
            if let Some(grouped) = &mut palette.grouped {
                grouped.choose_when_ready = true;
            }
            None
        } else {
            Some(palette.selected)
        }
    });
    if let Some(selected) = selected.flatten() {
        choose(selected);
    }
}

/// Arrows move through the list, Return runs, Escape closes; whether the command was handled.
fn command_key(command: Sel) -> bool {
    let step: isize = if command == sel!(moveUp:) {
        -1
    } else if command == sel!(moveDown:) {
        1
    } else if command == sel!(insertNewline:) {
        confirm_selection();
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
    let search = with_open(|palette| {
        let query = palette.field.stringValue().to_string();
        if let Some(grouped) = &mut palette.grouped {
            if grouped.query.as_ref() != Some(&query) {
                grouped.query = Some(query.clone());
                grouped.choose_when_ready = false;
                palette.found.clear();
                palette.selected = 0;
                palette.first = 0;
                palette.nothing.setStringValue(&ns(t("Lädt …")));
            }
            return (!query.trim().is_empty()).then(|| (grouped.search.clone(), query));
        }
        let entries: Vec<(&str, &str)> = palette
            .entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.hidden.as_str()))
            .collect();
        palette.found = model::rank(&query, &entries);
        palette.selected = 0;
        palette.first = 0;
        None
    });
    show();
    if let Some((search, query)) = search.flatten() {
        search(query);
    }
}

/// Refreshes the query of an open grouped palette when its background index changes.
pub fn refresh_grouped(cancel: &Cancel) {
    if !cancel.is_cancelled() {
        filter();
    }
}

/// Displays the groups from a background search if its palette and query are still current.
pub fn show_grouped(
    cancel: &Cancel,
    query: &str,
    groups: Vec<Group>,
    loading: bool,
    chosen: Rc<dyn Fn(usize)>,
    icon: ResultIcon,
) {
    if cancel.is_cancelled() || query.trim().is_empty() {
        return;
    }
    let confirm = with_open(|palette| {
        let Some(grouped) = &mut palette.grouped else {
            return false;
        };
        if grouped.query.as_deref() != Some(query) {
            return false;
        }
        palette.entries.clear();
        grouped.headings.clear();
        for group in groups {
            let title = group.title.map(Rc::<str>::from);
            grouped.headings.extend(std::iter::repeat_n(title, group.entries.len()));
            palette.entries.extend(group.entries);
        }
        palette.chosen = chosen;
        grouped.icon = Some(icon);
        palette.found = (0..palette.entries.len()).collect();
        palette.selected = palette.selected.min(palette.found.len().saturating_sub(1));
        palette
            .nothing
            .setStringValue(&ns(if loading { t("Lädt …") } else { t("Keine Treffer") }));
        let confirm = grouped.choose_when_ready && !palette.found.is_empty();
        if confirm || !loading {
            grouped.choose_when_ready = false;
        }
        confirm
    });
    show();
    if confirm == Some(true) {
        confirm_selection();
    }
}

fn heading(grouped: &Grouped, index: usize) -> Option<&Rc<str>> {
    grouped.headings.get(index).and_then(Option::as_ref)
}

fn starts_group(grouped: &Grouped, first: usize, index: usize) -> bool {
    heading(grouped, index).is_some_and(|title| {
        index == first || heading(grouped, index - 1).is_none_or(|previous| !Rc::ptr_eq(title, previous))
    })
}

fn viewport(palette: &Palette) -> (usize, usize) {
    let Some(grouped) = &palette.grouped else {
        return (
            model::first_shown(palette.first, palette.selected, SHOWN, palette.found.len()),
            SHOWN,
        );
    };
    let height = palette
        .root
        .window()
        .and_then(|window| window.contentView())
        .map(|content| content.safeAreaLayoutGuide().frame().size.height - 20.0);
    let height = height
        .filter(|height| *height > 0.0)
        .unwrap_or(quick_open::HEIGHT)
        .min(quick_open::HEIGHT);
    let mut first = palette.first.min(palette.selected);
    loop {
        let mut available = height - 76.0;
        let mut count = 0;
        for index in (first..palette.found.len()).take(SHOWN) {
            let group_space = if starts_group(grouped, first, index) { 16.0 } else { 0.0 };
            available -= quick_open::ROW_HEIGHT + group_space;
            if available < 0.0 && count > 0 {
                break;
            }
            count += 1;
        }
        if count == 0 || palette.selected < first + count {
            return (first, count);
        }
        first += 1;
    }
}

fn paint(row: &Row, entry: &Entry, selected: bool, grouped: Option<&Grouped>) {
    row.shortcut.setStringValue(&ns(&entry.shortcut));
    row.view.setAccessibilitySelected(selected);
    let background = if let Some(grouped) = grouped {
        let image = grouped.icon.as_ref().and_then(|icon| icon(entry.tag));
        if let Some(icon) = &row.icon {
            icon.setImage(image.as_deref());
        }
        quick_open::show_text(&row.title, &entry.title, grouped.query.as_deref().unwrap_or(""));
        row.shortcut.setTextColor(Some(&NSColor::secondaryLabelColor()));
        NSColor::unemphasizedSelectedContentBackgroundColor()
    } else {
        row.title.setStringValue(&ns(&entry.title));
        let (title, secondary) = if selected {
            (
                NSColor::alternateSelectedControlTextColor(),
                NSColor::alternateSelectedControlTextColor(),
            )
        } else {
            (NSColor::labelColor(), NSColor::secondaryLabelColor())
        };
        row.title.setTextColor(Some(&title));
        row.shortcut.setTextColor(Some(&secondary));
        NSColor::selectedContentBackgroundColor()
    };
    if let Some(layer) = row.view.layer() {
        let color = if selected { background } else { NSColor::clearColor() };
        layer.setBackgroundColor(Some(&color.CGColor()));
    }
}

/// Shows the rows around the selected entry.
fn show() {
    with_open(|palette| {
        let (first, room) = viewport(palette);
        palette.first = first;
        let grouped = palette.grouped.as_ref();
        for (at, row) in palette.rows.iter().enumerate() {
            let index = first + at;
            let Some(entry) = palette
                .found
                .get(index)
                .filter(|_| at < room)
                .map(|entry| &palette.entries[*entry])
            else {
                row.heading.setHidden(true);
                row.view.setHidden(true);
                continue;
            };
            let title = grouped.and_then(|grouped| heading(grouped, index));
            let heading_visible = grouped.is_some_and(|grouped| starts_group(grouped, first, index));
            row.heading.setHidden(!heading_visible);
            if let Some(title) = title.filter(|_| heading_visible) {
                row.heading.setStringValue(&ns(title));
            }
            row.view.setHidden(false);
            let spoken = [
                entry.title.as_str(),
                entry.shortcut.as_str(),
                title.map_or("", Rc::as_ref),
            ]
            .iter()
            .filter(|part| !part.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
            row.view.setAccessibilityLabel(Some(&ns(&spoken)));
            paint(row, entry, index == palette.selected, grouped);
        }
        let blank = grouped.is_some_and(|grouped| grouped.query.as_deref().is_none_or(|query| query.trim().is_empty()));
        palette.nothing.setHidden(blank || !palette.found.is_empty());
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
    if run {
        confirm_selection();
    }
    titles
}
