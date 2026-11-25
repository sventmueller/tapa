//! The bar below an editor: a passing remark, such as what Vim says, and the editor's own controls, Vim's mode and the
//! indentation.

use std::time::Duration;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{
    NSBox, NSBoxType, NSColor, NSLayoutConstraintOrientation, NSLineBreakMode, NSStackView, NSStackViewGravity,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSEdgeInsets;
use term_ui::kit::style;

use super::view::EditorTextView;
use crate::util::{activate, label, main_after, ns, pin_inset};

/// The bar's height in points.
pub(crate) const HEIGHT: f64 = 24.0;
/// How long a remark stays.
const NOTE: Duration = Duration::from_secs(6);

/// The controls of one editor's bar.
pub(crate) struct Bar {
    note: Retained<NSTextField>,
}

/// The bar of `view`, with a line above it.
pub(crate) fn bar(mtm: MainThreadMarker, view: &EditorTextView) -> Retained<NSView> {
    let note = label(mtm, "");
    term_ui::kit::ui_size::set_system_font(&note, style::SMALL);
    note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    note.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    note.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setSpacing(style::space::L);
    stack.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 10.0,
        bottom: 0.0,
        right: 10.0,
    });
    stack.addView_inGravity(&note, NSStackViewGravity::Leading);
    super::status_items(mtm, view, &stack);
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    let container = NSView::new(mtm);
    container.addSubview(&line);
    container.addSubview(&stack);
    line.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        line.leadingAnchor().constraintEqualToAnchor(&container.leadingAnchor()),
        line.trailingAnchor()
            .constraintEqualToAnchor(&container.trailingAnchor()),
        line.topAnchor().constraintEqualToAnchor(&container.topAnchor()),
    ]);
    let below_line = NSEdgeInsets {
        top: 1.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };
    pin_inset(&stack, &container, below_line);
    *view.ivars().around.status.borrow_mut() = Some(Bar { note });
    container
}

/// Shows `text` in the bar of `view` for a few seconds.
pub(crate) fn note(view: &EditorTextView, text: &str) {
    let shown = view.ivars().around.status.borrow().as_ref().map(|bar| bar.note.clone());
    let Some(field) = shown else { return };
    field.setStringValue(&ns(text));
    let serial = view.ivars().serial;
    let said = text.to_string();
    main_after(NOTE, move || {
        let Some(view) = crate::find_editor(serial) else { return };
        let shown = view.ivars().around.status.borrow().as_ref().map(|bar| bar.note.clone());
        if let Some(field) = shown
            && field.stringValue().to_string() == said
        {
            field.setStringValue(&ns(""));
        }
    });
}
