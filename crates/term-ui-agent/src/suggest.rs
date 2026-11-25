//! The list of completions for an `@` mention, above the composer: files of the project, the terminal, the browser.

use std::cell::{Cell, RefCell};
use std::ops::Range;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{NSAccessibility, NSBox, NSButton, NSCellImagePosition, NSColor, NSStackView, NSView};
use splat_i18n::t;

use super::style;
use super::target::AgentTarget;
use super::ui;
use crate::common::util::{ns, set_action, symbol};

/// How many completions show at most.
pub(crate) const SHOWN: usize = 8;

/// One completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) detail: String,
    /// What replaces the typed mention.
    pub(crate) insert: String,
    pub(crate) symbol: &'static str,
}

pub(crate) struct Suggestions {
    pub(crate) view: Retained<NSBox>,
    column: Retained<NSStackView>,
    target: Retained<AgentTarget>,
    items: RefCell<Vec<Item>>,
    /// The rows in the list, with the item each shows and whether it shows it chosen.
    rows: RefCell<Vec<(Item, bool, Retained<NSBox>)>>,
    selected: Cell<usize>,
    /// The bytes of the composer's text the chosen completion replaces.
    range: RefCell<Range<usize>>,
    /// Counts the questions to the file index, so a late answer to an older one is dropped.
    asked: Cell<u64>,
}

impl Suggestions {
    pub(crate) fn new(mtm: MainThreadMarker, target: &AgentTarget) -> Suggestions {
        let column = ui::column(mtm, 0.0, &[]);
        let view = ui::framed(
            mtm,
            &column,
            &NSColor::separatorColor(),
            Some(&NSColor::controlBackgroundColor()),
            8.0,
            ui::insets(4.0, 4.0, 4.0, 4.0),
        );
        view.setHidden(true);
        column.setAccessibilityLabel(Some(&ns(t("Vorschläge"))));
        Suggestions {
            view,
            column,
            target: objc2::Message::retain(target),
            items: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
            selected: Cell::new(0),
            range: RefCell::new(0..0),
            asked: Cell::new(0),
        }
    }

    /// A new question to the file index; answers to earlier ones are dropped.
    pub(crate) fn ask(&self) -> u64 {
        self.asked.set(self.asked.get() + 1);
        self.asked.get()
    }

    pub(crate) fn is_current(&self, asked: u64) -> bool {
        self.asked.get() == asked
    }

    pub(crate) fn is_shown(&self) -> bool {
        !self.view.isHidden()
    }

    /// Shows `items` for the mention at `range`, the first chosen.
    pub(crate) fn show(&self, items: Vec<Item>, range: Range<usize>) {
        *self.range.borrow_mut() = range;
        let items: Vec<Item> = items.into_iter().take(SHOWN).collect();
        let empty = items.is_empty();
        *self.items.borrow_mut() = items;
        self.selected.set(0);
        self.draw();
        self.view.setHidden(empty);
    }

    /// Hides the list; answers still on their way are dropped.
    pub(crate) fn hide(&self) {
        self.ask();
        self.view.setHidden(true);
        self.items.borrow_mut().clear();
    }

    /// Moves the choice by `by` rows, wrapping around.
    pub(crate) fn step(&self, by: isize) {
        let count = self.items.borrow().len();
        if count == 0 {
            return;
        }
        let count = isize::try_from(count).unwrap_or(1);
        let next = (isize::try_from(self.selected.get()).unwrap_or(0) + by).rem_euclid(count);
        self.selected.set(usize::try_from(next).unwrap_or(0));
        self.draw();
    }

    /// Chooses row `index`.
    pub(crate) fn choose(&self, index: usize) {
        if index < self.items.borrow().len() {
            self.selected.set(index);
        }
    }

    /// The chosen completion and the range it replaces.
    pub(crate) fn chosen(&self) -> Option<(Item, Range<usize>)> {
        let item = self.items.borrow().get(self.selected.get()).cloned()?;
        Some((item, self.range.borrow().clone()))
    }

    /// Brings the rows to the items, building only those whose item changed.
    fn draw(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let selected = self.selected.get();
        let items = self.items.borrow();
        let mut rows = self.rows.borrow_mut();
        let kept = items.len().min(rows.len());
        for old in rows.drain(kept..) {
            self.column.removeArrangedSubview(&old.2);
            old.2.removeFromSuperview();
        }
        for (n, item) in items.iter().enumerate() {
            let chosen = n == selected;
            match rows.get_mut(n) {
                Some((shown, was, frame)) if shown == item => {
                    if *was != chosen {
                        *was = chosen;
                        frame.setFillColor(&fill(chosen));
                    }
                }
                Some(slot) => {
                    let frame = self.row(mtm, item, n, chosen);
                    self.column.removeArrangedSubview(&slot.2);
                    slot.2.removeFromSuperview();
                    self.column
                        .insertArrangedSubview_atIndex(&frame, isize::try_from(n).unwrap_or(isize::MAX));
                    full_width(&self.column, &frame);
                    *slot = (item.clone(), chosen, frame);
                }
                None => {
                    let frame = self.row(mtm, item, n, chosen);
                    ui::add_full_width(&self.column, &frame);
                    rows.push((item.clone(), chosen, frame));
                }
            }
        }
    }

    fn row(&self, mtm: MainThreadMarker, item: &Item, index: usize, chosen: bool) -> Retained<NSBox> {
        // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
        let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(&item.label), None, None, mtm) };
        set_action(&button, &self.target, sel!(ideAgentSuggest:));
        button.setTag(isize::try_from(index).unwrap_or(-1));
        button.setBordered(false);
        button.setImage(symbol(item.symbol, &item.label).as_deref());
        button.setImagePosition(NSCellImagePosition::ImageLeading);
        button.setFont(Some(&style::chat_body()));
        crate::common::util::set_label(&button, Some(&format!("{} {}", item.label, item.detail)));
        ui::hug(&button);
        let detail = ui::label(mtm, &item.detail, &style::chat_small(), &NSColor::secondaryLabelColor());
        let spacer = ui::spacer(mtm);
        let line = ui::row(mtm, 6.0, &[&button, &detail, &spacer]);
        let frame = ui::framed(
            mtm,
            &line,
            &NSColor::clearColor(),
            Some(&fill(chosen)),
            5.0,
            ui::insets(2.0, 6.0, 2.0, 6.0),
        );
        frame.setBorderWidth(0.0);
        frame
    }
}

/// The background of a row, marked when it is the chosen one.
fn fill(chosen: bool) -> Retained<NSColor> {
    if chosen {
        NSColor::selectedContentBackgroundColor().colorWithAlphaComponent(0.18)
    } else {
        NSColor::clearColor()
    }
}

/// Holds `view`, which `stack` arranges, as wide as the stack.
fn full_width(stack: &NSStackView, view: &NSView) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor()
        .constraintEqualToAnchor(&stack.widthAnchor())
        .setActive(true);
}

#[cfg(feature = "selftest")]
mod selftest_probe;
