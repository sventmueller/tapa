//! The sidebar's foot: the settings button at its bottom left, as in Dango, under the list and under a panel alike.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{NSButton, NSColor, NSView};
use splat_i18n::t;
use term_ui::kit::{self, style, ui_size};

/// The button's side, a sidebar row's height.
const SIZE: f64 = 28.0;
/// How far the button keeps from the sidebar's leading and bottom edge.
const INSET: f64 = 6.0;
/// The room between what the sidebar shows and the button.
const GAP: f64 = 2.0;

/// Lays `views` out in `container` above a settings button that sends `action` to `target`, pinned to the bottom
/// left inside the safe area; the button.
pub(crate) fn place(
    mtm: MainThreadMarker,
    container: &NSView,
    views: &[&NSView],
    target: &AnyObject,
    action: Sel,
) -> Retained<NSButton> {
    let button = ui_size::symbol_button(
        mtm,
        "gearshape",
        t("Einstellungen"),
        style::symbol::LARGE,
        Some(target),
        action,
    );
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    kit::set_label(&button, Some(t("Einstellungen")));
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    container.addSubview(&button);
    let safe = container.safeAreaLayoutGuide();
    let sides = [
        button.widthAnchor().constraintEqualToConstant(SIZE),
        button.heightAnchor().constraintEqualToConstant(SIZE),
    ];
    for side in &sides {
        ui_size::follow_length(side, SIZE);
    }
    let mut constraints = vec![
        button
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&safe.leadingAnchor(), INSET),
        button
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&safe.bottomAnchor(), -INSET),
    ];
    constraints.extend(sides);
    for view in views {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        container.addSubview(view);
        constraints.extend([
            view.topAnchor().constraintEqualToAnchor(&container.topAnchor()),
            view.leadingAnchor().constraintEqualToAnchor(&container.leadingAnchor()),
            view.trailingAnchor()
                .constraintEqualToAnchor(&container.trailingAnchor()),
            view.bottomAnchor()
                .constraintEqualToAnchor_constant(&button.topAnchor(), -GAP),
        ]);
    }
    kit::activate(constraints);
    button
}
