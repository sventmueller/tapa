//! A bar above the terminal with a short message, an optional button and a close button, hidden until needed.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSImageView, NSLayoutAttribute, NSLineBreakMode, NSStackView,
    NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSSize};
use splat_i18n::t;

use crate::kit::{label, ns, pin_edges, set_action, symbol, symbol_button};

pub(crate) struct Banner {
    root: Retained<NSView>,
    message: Retained<NSTextField>,
}

impl Banner {
    /// A banner with the symbol `symbol_name` in `tint` at full strength on a background of `tint`, whose button, if
    /// any, sends its action and whose close button sends `close` to `target`.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        symbol_name: &str,
        tint: &NSColor,
        target: &AnyObject,
        button: Option<(&str, Sel)>,
        close: Sel,
    ) -> Banner {
        let background = NSBox::new(mtm);
        background.setBoxType(NSBoxType::Custom);
        background.setTitlePosition(NSTitlePosition::NoTitle);
        background.setBorderWidth(0.0);
        background.setCornerRadius(0.0);
        background.setContentViewMargins(NSSize::new(0.0, 0.0));
        background.setFillColor(tint);
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(crate::kit::space::M);
        row.setEdgeInsets(NSEdgeInsets {
            top: 5.0,
            left: 10.0,
            bottom: 5.0,
            right: 8.0,
        });
        if let Some(image) = symbol(symbol_name, "") {
            let icon = NSImageView::imageViewWithImage(&image, mtm);
            icon.setContentTintColor(Some(&tint.colorWithAlphaComponent(1.0)));
            row.addArrangedSubview(&icon);
        }
        let message = label(mtm, "");
        message.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        message.setContentCompressionResistancePriority_forOrientation(
            1.0,
            objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
        );
        row.addArrangedSubview(&message);
        let spacer = NSView::new(mtm);
        spacer.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
        row.addArrangedSubview(&spacer);
        if let Some((title, action)) = button {
            // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
            let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
            set_action(&button, target, action);
            button.setControlSize(NSControlSize::Small);
            row.addArrangedSubview(&button);
        }
        let dismiss = symbol_button(mtm, "xmark", t("Schließen"), 10.0, Some(target), close);
        dismiss.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        row.addArrangedSubview(&dismiss);
        let root = NSView::new(mtm);
        root.addSubview(&background);
        pin_edges(&background, &root);
        root.addSubview(&row);
        pin_edges(&row, &root);
        let line = NSBox::new(mtm);
        line.setBoxType(NSBoxType::Separator);
        root.addSubview(&line);
        crate::kit::pin_sides(&line, &root, 0.0);
        crate::kit::activate([line.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor())]);
        root.setHidden(true);
        Banner { root, message }
    }

    pub(crate) fn view(&self) -> &NSView {
        &self.root
    }

    pub(crate) fn show(&self, text: &str) {
        self.message.setStringValue(&ns(text));
        self.root.setHidden(false);
    }

    pub(crate) fn hide(&self) {
        self.root.setHidden(true);
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
