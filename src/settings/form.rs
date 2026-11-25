//! The controls of the settings window, each bound to a field: how it shows the settings in force and what it writes
//! into the file when the user changes it. A control's tag is its field's place in the list.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message, sel};
use objc2_app_kit::{
    NSButton, NSControl, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSGridCellPlacement, NSGridView,
    NSPopUpButton, NSScrollView, NSTextField, NSView,
};
use objc2_foundation::{NSArray, NSRect};
use term_config::{Config, Document};
use term_ui::kit::{label, ns, set_action, space};

/// A control's value: on or off, or a text, for a pop-up the value of the item chosen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Shown {
    /// A checkbox.
    Bool(bool),
    /// A text field, a label or a pop-up.
    Text(String),
}

impl Shown {
    pub(crate) fn text(&self) -> &str {
        match self {
            Shown::Text(text) => text,
            Shown::Bool(_) => "",
        }
    }

    pub(crate) fn on(&self) -> bool {
        matches!(self, Shown::Bool(true))
    }
}

/// What the settings in force show in a control.
pub(crate) type Read = Box<dyn Fn(&Config) -> Shown>;
/// What a control's new value changes in the file; `false` when it changed nothing there.
pub(crate) type Write = Rc<dyn Fn(&mut Document, &Shown) -> Result<bool, String>>;
/// The items of a pop-up, title and value.
pub(crate) type Items = Box<dyn Fn(&Config) -> Vec<(String, String)>>;
/// Whether the settings in force let a control be changed.
type When = Box<dyn Fn(&Config) -> bool>;

enum Control {
    Check(Retained<NSButton>),
    Text(Retained<NSTextField>),
    Label(Retained<NSTextField>),
    Popup {
        button: Retained<NSPopUpButton>,
        items: Items,
        values: RefCell<Vec<String>>,
    },
}

/// A control and its field.
struct Field {
    control: Control,
    read: Read,
    write: Option<Write>,
    enabled: Option<When>,
}

/// Every field of the window.
pub(crate) struct Fields {
    mtm: MainThreadMarker,
    target: Retained<AnyObject>,
    list: RefCell<Vec<Field>>,
}

impl Fields {
    pub(crate) fn new(mtm: MainThreadMarker, target: Retained<AnyObject>) -> Fields {
        Fields {
            mtm,
            target,
            list: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn mtm(&self) -> MainThreadMarker {
        self.mtm
    }

    pub(crate) fn target(&self) -> &AnyObject {
        &self.target
    }

    fn add(&self, control: Control, read: Read, write: Option<Write>) -> isize {
        let mut list = self.list.borrow_mut();
        let tag = isize::try_from(list.len()).unwrap_or(isize::MAX);
        let ns_control: Option<&NSControl> = match &control {
            Control::Check(button) => Some(button),
            Control::Text(field) => Some(field),
            Control::Popup { button, .. } => Some(button),
            Control::Label(_) => None,
        };
        if let Some(ns_control) = ns_control {
            ns_control.setTag(tag);
            set_action(ns_control, &self.target, sel!(termSettingsChanged:));
        }
        list.push(Field {
            control,
            read,
            write,
            enabled: None,
        });
        tag
    }

    /// Has the field `tag` enabled only while `when` holds.
    pub(crate) fn enable_when(&self, tag: isize, when: impl Fn(&Config) -> bool + 'static) {
        let mut list = self.list.borrow_mut();
        if let Some(field) = usize::try_from(tag).ok().and_then(|at| list.get_mut(at)) {
            field.enabled = Some(Box::new(when));
        }
    }

    /// A checkbox titled `title`.
    pub(crate) fn check(&self, title: &str, read: Read, write: Write) -> (Retained<NSButton>, isize) {
        // SAFETY: A nil target and no action until `add` gives it both.
        let button = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, self.mtm) };
        let tag = self.add(Control::Check(button.clone()), read, Some(write));
        (button, tag)
    }

    /// A text field `width` points wide that writes when Return is pressed or it loses the keyboard.
    pub(crate) fn text(&self, width: f64, read: Read, write: Write) -> (Retained<NSTextField>, isize) {
        let field = NSTextField::textFieldWithString(&ns(""), self.mtm);
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(true);
        }
        field.widthAnchor().constraintEqualToConstant(width).setActive(true);
        let tag = self.add(Control::Text(field.clone()), read, Some(write));
        (field, tag)
    }

    /// A label whose text follows the settings, such as where an inherited value comes from.
    pub(crate) fn note(&self, read: Read) -> Retained<NSTextField> {
        let note = secondary(self.mtm, "");
        self.add(Control::Label(note.clone()), read, None);
        note
    }

    /// A pop-up of `items`, which may change with the settings.
    pub(crate) fn popup(&self, items: Items, read: Read, write: Write) -> (Retained<NSPopUpButton>, isize) {
        let button = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(self.mtm), NSRect::ZERO, false);
        let control = Control::Popup {
            button: button.clone(),
            items,
            values: RefCell::new(Vec::new()),
        };
        let tag = self.add(control, read, Some(write));
        (button, tag)
    }

    /// The value the control `tag` holds now, and what writes it; `None` while it still shows what `config` says,
    /// as a text field does that only lost the keyboard, which must not turn an inherited value into an own one.
    pub(crate) fn taken(&self, tag: isize, config: &Config) -> Option<(Shown, Write)> {
        let list = self.list.borrow();
        let field = list.get(usize::try_from(tag).ok()?)?;
        let shown = match &field.control {
            Control::Check(button) => Shown::Bool(button.state() == NSControlStateValueOn),
            Control::Text(text) => Shown::Text(text.stringValue().to_string()),
            Control::Label(_) => return None,
            Control::Popup { button, values, .. } => {
                let at = usize::try_from(button.indexOfSelectedItem()).ok()?;
                Shown::Text(values.borrow().get(at)?.clone())
            }
        };
        if shown == (field.read)(config) {
            return None;
        }
        Some((shown, field.write.clone()?))
    }

    /// Shows `config` in every control, except a text field the user is typing in unless it is `force`.
    pub(crate) fn refresh(&self, config: &Config, force: Option<isize>) {
        for (at, field) in self.list.borrow().iter().enumerate() {
            let forced = force.is_some_and(|tag| usize::try_from(tag).ok() == Some(at));
            let shown = (field.read)(config);
            let enabled = field.enabled.as_ref().is_none_or(|when| when(config));
            match &field.control {
                Control::Check(button) => {
                    button.setState(if shown.on() {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                    button.setEnabled(enabled);
                }
                Control::Text(text) => {
                    text.setEnabled(enabled);
                    if forced || text.currentEditor().is_none() {
                        text.setStringValue(&ns(shown.text()));
                    }
                }
                Control::Label(note) => {
                    note.setStringValue(&ns(shown.text()));
                    hide_row(note, shown.text().is_empty());
                }
                Control::Popup { button, items, values } => {
                    self.fill(button, &items(config), values, shown.text());
                    button.setEnabled(enabled);
                }
            }
        }
    }

    /// Puts `items` into `button` and chooses `chosen`; a value the file names that is not among them gets an item of
    /// its own, so the pop-up never shows something the file does not say.
    fn fill(&self, button: &NSPopUpButton, items: &[(String, String)], values: &RefCell<Vec<String>>, chosen: &str) {
        let mut items = items.to_vec();
        if !items.iter().any(|(_, value)| value == chosen) {
            items.push((chosen.to_string(), chosen.to_string()));
        }
        let wanted: Vec<String> = items.iter().map(|(_, value)| value.clone()).collect();
        if *values.borrow() != wanted {
            button.removeAllItems();
            if let Some(menu) = button.menu() {
                for (title, _) in &items {
                    menu.addItem(&term_ui::kit::item(self.mtm, title, None, ""));
                }
            }
            *values.borrow_mut() = wanted;
        }
        if let Some(at) = values.borrow().iter().position(|value| value == chosen) {
            button.selectItemAtIndex(isize::try_from(at).unwrap_or(0));
        }
    }
}

/// How wide explanatory text under a control grows before it wraps.
const HINT_WIDTH: f64 = 340.0;
/// The narrowest a form's column of titles gets, so that the panes line up their controls.
const TITLES: f64 = 150.0;
/// The tallest a pane gets before it scrolls.
pub(crate) const MAX_HEIGHT: f64 = 600.0;

/// Small secondary text under a control, which explains it, as macOS settings put it.
pub(crate) fn secondary(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let hint = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    hint.setTextColor(Some(&objc2_app_kit::NSColor::secondaryLabelColor()));
    hint.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    hint.setPreferredMaxLayoutWidth(HINT_WIDTH);
    hint.setSelectable(false);
    hint
}

/// What a row of a form holds.
enum Line {
    /// A title, empty for none, and the view beside it.
    Row(Option<Retained<NSTextField>>, Retained<NSView>),
    /// A section's heading across both columns.
    Heading(Retained<NSTextField>),
    /// A view across both columns.
    Wide(Retained<NSView>),
}

/// Rows of a label and a control, laid out as macOS settings are: labels right-aligned before their controls, a
/// group's label beside its first row, space between the groups and a bold heading before a section.
pub(crate) struct Form {
    mtm: MainThreadMarker,
    rows: Vec<Line>,
    gaps: Vec<usize>,
    hints: Vec<usize>,
}

impl Form {
    pub(crate) fn new(mtm: MainThreadMarker) -> Form {
        Form {
            mtm,
            rows: Vec::new(),
            gaps: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// A row of `title`, empty for none, and `view`; VoiceOver names an editable control by its title.
    pub(crate) fn row(&mut self, title: &str, view: &NSView) {
        let title = (!title.is_empty()).then(|| label(self.mtm, title));
        let named = view.downcast_ref::<NSPopUpButton>().is_some()
            || view.downcast_ref::<NSTextField>().is_some_and(NSTextField::isEditable);
        if let (Some(title), Some(control)) = (&title, view.downcast_ref::<NSControl>())
            && named
        {
            term_ui::kit::set_title_element(control, title);
        }
        self.rows.push(Line::Row(title, view.retain()));
    }

    /// Space before the next row, which starts another group.
    pub(crate) fn gap(&mut self) {
        self.gaps.push(self.rows.len());
    }

    /// A bold heading before the next rows, which start a section.
    pub(crate) fn heading(&mut self, title: &str) {
        let heading = label(self.mtm, title);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
        self.gaps.push(self.rows.len());
        self.rows.push(Line::Heading(heading));
    }

    /// `view` across both columns, such as a list.
    pub(crate) fn wide(&mut self, view: &NSView) {
        self.rows.push(Line::Wide(view.retain()));
    }

    /// Explanatory text under the row before, in the controls' column.
    pub(crate) fn hint(&mut self, text: &str) {
        let hint = secondary(self.mtm, text);
        self.under(&hint);
    }

    /// `view`, secondary text, under the row before, in the controls' column.
    pub(crate) fn under(&mut self, view: &NSView) {
        self.hints.push(self.rows.len());
        self.rows.push(Line::Row(None, view.retain()));
    }

    /// The grid of the rows.
    pub(crate) fn build(self) -> Retained<NSGridView> {
        let empty = || objc2_app_kit::NSGridCell::emptyContentView(self.mtm);
        let mut widest: f64 = TITLES;
        let rows: Vec<Retained<NSArray<NSView>>> = self
            .rows
            .iter()
            .map(|line| match line {
                Line::Row(title, view) => {
                    let title = title.clone().map_or_else(empty, |title| {
                        widest = widest.max(title.fittingSize().width);
                        Retained::into_super(Retained::into_super(title))
                    });
                    NSArray::from_retained_slice(&[title, view.clone()])
                }
                Line::Heading(heading) => NSArray::from_retained_slice(&[
                    Retained::into_super(Retained::into_super(heading.clone())),
                    empty(),
                ]),
                Line::Wide(view) => NSArray::from_retained_slice(&[view.clone(), empty()]),
            })
            .collect();
        let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), self.mtm);
        grid.setRowSpacing(space::M);
        grid.setColumnSpacing(space::M);
        grid.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Trailing);
        grid.columnAtIndex(0).setWidth(widest.ceil());
        grid.columnAtIndex(1).setXPlacement(NSGridCellPlacement::Leading);
        grid.setRowAlignment(objc2_app_kit::NSGridRowAlignment::FirstBaseline);
        for (at, line) in self.rows.iter().enumerate() {
            if matches!(line, Line::Heading(_) | Line::Wide(_)) {
                let row = grid.rowAtIndex(isize::try_from(at).unwrap_or(0));
                row.mergeCellsInRange(objc2_foundation::NSRange::new(0, 2));
                row.cellAtIndex(0).setXPlacement(NSGridCellPlacement::Leading);
            }
        }
        for &at in self.gaps.iter().filter(|at| **at > 0 && **at < self.rows.len()) {
            grid.rowAtIndex(isize::try_from(at).unwrap_or(0))
                .setTopPadding(space::GROUP - space::M);
        }
        // Explanatory text sits close under its control, as in macOS settings.
        for &at in &self.hints {
            grid.rowAtIndex(isize::try_from(at).unwrap_or(0))
                .setTopPadding(-space::XS);
        }
        grid
    }
}

/// `view` as a pane, scrolling once it is taller than `MAX_HEIGHT`.
pub(crate) fn scrolling(view: &NSView) -> Retained<NSView> {
    let height = view.fittingSize().height;
    if height <= MAX_HEIGHT {
        return view.retain();
    }
    let width = view.fittingSize().width;
    let scroll = NSScrollView::new(view.mtm());
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(view));
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let clip = scroll.contentView();
    term_ui::kit::activate([
        view.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
        view.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
        scroll.heightAnchor().constraintEqualToConstant(MAX_HEIGHT),
        scroll.widthAnchor().constraintEqualToConstant(width + SCROLLER),
    ]);
    Retained::into_super(scroll)
}

/// The room a pane that scrolls keeps beside it for the scroller.
const SCROLLER: f64 = 16.0;

/// Hides `view` and, where it sits in a form, its row, so an empty note leaves no gap.
fn hide_row(view: &NSView, hidden: bool) {
    view.setHidden(hidden);
    // SAFETY: Read on the main thread, where the view tree keeps the parent alive while the call runs.
    let parent = unsafe { view.superview() };
    let grid = parent.and_then(|parent| parent.downcast::<NSGridView>().ok());
    if let Some(cell) = grid.and_then(|grid| grid.cellForView(view))
        && let Some(row) = cell.row()
    {
        row.setHidden(hidden);
    }
}
