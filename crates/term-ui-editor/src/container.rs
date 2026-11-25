//! The editor with what surrounds it: line numbers, the status bar, the bar about its file and a Markdown
//! document's format bar above.

use std::cell::{Cell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker};

use super::view::EditorTextView;
use super::{conflict, find_editor, overlay, ruler, status, viewport, whitespace};
use crate::util::{activate, auto_layout, pin_sides};

/// The views an editor has around and over its text.
#[derive(Default)]
pub(crate) struct Around {
    /// The line number column, redrawn when lines change.
    pub(super) gutter: RefCell<Option<Weak<ruler::LineNumbers>>>,
    /// The view that shows its whitespace.
    pub(super) whitespace: RefCell<Option<Weak<whitespace::Overlay>>>,
    /// What is drawn over the text: fold placeholders and the further carets.
    pub(super) overlay: RefCell<Option<Weak<overlay::Overlay>>>,
    /// The bar above the editor.
    pub(super) bar: RefCell<Option<conflict::Bar>>,
    /// The format bar of a Markdown document, at the top.
    pub(crate) format: RefCell<Option<crate::markdown::FormatBar>>,
    /// The status bar below the editor.
    pub(super) status: RefCell<Option<status::Bar>>,
    /// The line number column draws again on the next turn of the run loop.
    pub(super) due: Cell<bool>,
    /// The split between the text and what the app shows beside it, such as a Markdown preview.
    pub(super) split: RefCell<Option<Weak<objc2_app_kit::NSSplitView>>>,
}

/// The scroll view with the line numbers beside it and the status bar below.
pub(super) fn editor_container(
    mtm: MainThreadMarker,
    scroll: &objc2_app_kit::NSScrollView,
    view: &EditorTextView,
) -> (Retained<objc2_app_kit::NSView>, Retained<ruler::LineNumbers>) {
    let (column, gutter) = ruler::column(mtm, view);
    let status = status::bar(mtm, view);
    let bar = conflict::Bar::new(mtm);
    let format = crate::markdown::FormatBar::new(mtm);
    let container = objc2_app_kit::NSView::new(mtm);
    let body = objc2_app_kit::NSView::new(mtm);
    body.addSubview(&column);
    body.addSubview(scroll);
    let split = objc2_app_kit::NSSplitView::new(mtm);
    split.setVertical(true);
    split.setDividerStyle(objc2_app_kit::NSSplitViewDividerStyle::Thin);
    split.addSubview(&body);
    container.addSubview(&format.view);
    container.addSubview(&split);
    container.addSubview(&status);
    container.addSubview(bar.view());
    viewport::follow(scroll, view);
    auto_layout(&[bar.view(), &column, scroll, &status, &split]);
    pin_sides(bar.view(), &container, 0.0);
    pin_sides(&format.view, &container, 0.0);
    pin_sides(&status, &container, 0.0);
    pin_sides(&split, &container, 0.0);
    activate([
        format.view.topAnchor().constraintEqualToAnchor(&container.topAnchor()),
        bar.view()
            .topAnchor()
            .constraintEqualToAnchor(&format.view.bottomAnchor()),
        split.topAnchor().constraintEqualToAnchor(&bar.view().bottomAnchor()),
        split.bottomAnchor().constraintEqualToAnchor(&status.topAnchor()),
        // Code and its line numbers stay left to right in a right-to-left interface (E3); the bars around follow it.
        column.leftAnchor().constraintEqualToAnchor(&body.leftAnchor()),
        column.topAnchor().constraintEqualToAnchor(&body.topAnchor()),
        column.bottomAnchor().constraintEqualToAnchor(&body.bottomAnchor()),
        scroll.leftAnchor().constraintEqualToAnchor(&column.rightAnchor()),
        scroll.rightAnchor().constraintEqualToAnchor(&body.rightAnchor()),
        scroll.topAnchor().constraintEqualToAnchor(&body.topAnchor()),
        scroll.bottomAnchor().constraintEqualToAnchor(&body.bottomAnchor()),
        status.bottomAnchor().constraintEqualToAnchor(&container.bottomAnchor()),
    ]);
    let status_height = status.heightAnchor().constraintEqualToConstant(status::HEIGHT);
    term_ui::kit::ui_size::follow_length(&status_height, status::HEIGHT);
    status_height.setActive(true);
    *view.ivars().around.split.borrow_mut() = Some(Weak::from(&*split));
    let width = column.widthAnchor().constraintEqualToConstant(0.0);
    width.setActive(true);
    gutter.set_width_constraint(width);
    *view.ivars().around.bar.borrow_mut() = Some(bar);
    *view.ivars().around.format.borrow_mut() = Some(format);
    (container, gutter)
}

/// Redraws the line number column of `view`, after its diagnostics changed, once for any number of asks in a turn of
/// the run loop.
pub(crate) fn redraw_gutter(view: &EditorTextView) {
    if view.ivars().around.due.replace(true) {
        return;
    }
    let serial = view.ivars().serial;
    crate::util::main_soon(move || {
        if let Some(view) = find_editor(serial) {
            redraw_gutter_now(&view);
        }
    });
}

fn redraw_gutter_now(view: &EditorTextView) {
    view.ivars().around.due.set(false);
    if let Some(gutter) = view.ivars().around.gutter.borrow().as_ref().and_then(Weak::load) {
        gutter.redraw();
    }
}

/// Shows `beside` to the right of the text of `view`, half as wide as both, or takes away what showed there.
pub(crate) fn set_beside(view: &EditorTextView, beside: Option<&objc2_app_kit::NSView>) {
    let Some(split) = view.ivars().around.split.borrow().as_ref().and_then(Weak::load) else {
        return;
    };
    for old in split.subviews().iter().skip(1) {
        old.removeFromSuperview();
    }
    let Some(beside) = beside else {
        split.adjustSubviews();
        return;
    };
    beside.setTranslatesAutoresizingMaskIntoConstraints(true);
    split.addSubview(beside);
    split.adjustSubviews();
    let width = split.frame().size.width;
    split.setPosition_ofDividerAtIndex((width / 2.0).round(), 0);
}
