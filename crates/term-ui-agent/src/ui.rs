//! Small building blocks of the agent tab's cards and bars.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityGroupRole, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControlSize,
    NSFont, NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageView, NSLayoutAttribute,
    NSLayoutConstraintOrientation, NSLineBreakMode, NSProgressIndicator, NSProgressIndicatorStyle, NSStackView,
    NSStackViewDistribution, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSSize};

use crate::common::constant;
use crate::common::util::{activate, fixed_size, ns, pin_edges, pin_inset, set_action, symbol};

/// A label on one line, cut at its end when it does not fit.
pub(crate) fn label(mtm: MainThreadMarker, text: &str, font: &NSFont, color: &NSColor) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&ns(text), mtm);
    field.setFont(Some(font));
    field.setTextColor(Some(color));
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    field
}

/// A label that wraps at `width`.
pub(crate) fn wrapping(
    mtm: MainThreadMarker,
    text: &str,
    font: &NSFont,
    color: &NSColor,
    width: f64,
) -> Retained<NSTextField> {
    let field = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    field.setFont(Some(font));
    field.setTextColor(Some(color));
    field.setPreferredMaxLayoutWidth(width.max(40.0));
    field.setSelectable(true);
    field
}

fn stack(mtm: MainThreadMarker, orientation: NSUserInterfaceLayoutOrientation, spacing: f64) -> Retained<NSStackView> {
    let stack = NSStackView::new(mtm);
    stack.setOrientation(orientation);
    stack.setSpacing(spacing);
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    stack
}

/// Views side by side, centred on one line.
pub(crate) fn row(mtm: MainThreadMarker, spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    let stack = stack(mtm, NSUserInterfaceLayoutOrientation::Horizontal, spacing);
    stack.setAlignment(NSLayoutAttribute::CenterY);
    for view in views {
        stack.addArrangedSubview(view);
    }
    stack
}

/// Views one below the other, each as wide as the column.
pub(crate) fn column(mtm: MainThreadMarker, spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    let stack = stack(mtm, NSUserInterfaceLayoutOrientation::Vertical, spacing);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setDistribution(NSStackViewDistribution::Fill);
    for view in views {
        add_full_width(&stack, view);
    }
    stack
}

/// Adds `view` to the vertical `stack`, as wide as the stack.
pub(crate) fn add_full_width(stack: &NSStackView, view: &NSView) {
    stack.addArrangedSubview(view);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor()
        .constraintEqualToAnchor(&stack.widthAnchor())
        .setActive(true);
}

/// A view that takes the room left in a row.
pub(crate) fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::new(mtm);
    view.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    view.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    view
}

/// `content` in a rounded box with a 1-point border, `padding` points inside.
pub fn framed(
    mtm: MainThreadMarker,
    content: &NSView,
    border: &NSColor,
    fill: Option<&NSColor>,
    radius: f64,
    padding: NSEdgeInsets,
) -> Retained<NSBox> {
    let frame = NSBox::new(mtm);
    frame.setBoxType(NSBoxType::Custom);
    frame.setTitlePosition(NSTitlePosition::NoTitle);
    frame.setBorderWidth(1.0);
    frame.setBorderColor(border);
    frame.setCornerRadius(radius);
    let fill = fill.map_or_else(NSColor::clearColor, |c| c.retain());
    frame.setFillColor(&fill);
    frame.setContentViewMargins(NSSize::new(0.0, 0.0));
    frame.setTranslatesAutoresizingMaskIntoConstraints(false);
    let inner = NSView::new(mtm);
    frame.setContentView(Some(&inner));
    // The box would place its content view by autoresizing offsets taken from its first frames, which are stale
    // when the card is measured; pinned, the box is exactly as tall as its content.
    pin_edges(&inner, &frame);
    inner.addSubview(content);
    pin_inset(content, &inner, padding);
    frame
}

/// Decoded pictures by a hash of their bytes.
pub(crate) type Pictures = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<u64, Retained<NSImage>>>>;

/// The side of a picture's tile, about as large as in Claude's app.
const TILE: f64 = 76.0;
const TILE_PADDING: f64 = 6.0;
const TILE_RADIUS: f64 = 10.0;

/// A picture as a tile showing it, named `name`, that opens it large on a click, with `remove` in its corner if it
/// can be dropped.
pub(crate) fn picture_tile(
    mtm: MainThreadMarker,
    data: &[u8],
    name: &str,
    remove: Option<&NSButton>,
    pictures: Option<&Pictures>,
) -> Retained<NSView> {
    let view = NSImageView::new(mtm);
    show_decoded(mtm, &view, data, pictures);
    view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    view.setAccessibilityElement(false);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let tile = framed(
        mtm,
        &view,
        &NSColor::separatorColor(),
        Some(&NSColor::controlBackgroundColor()),
        TILE_RADIUS,
        insets(TILE_PADDING, TILE_PADDING, TILE_PADDING, TILE_PADDING),
    );
    fixed_size(&tile, TILE, TILE);
    let Some(inner) = tile.contentView() else {
        return Retained::into_super(tile);
    };
    let enlarge = super::enlarge::button(mtm, name);
    inner.addSubview(&enlarge);
    pin_edges(&enlarge, &inner);
    if let Some(remove) = remove {
        inner.addSubview(remove);
        remove.setTranslatesAutoresizingMaskIntoConstraints(false);
        activate([
            remove
                .topAnchor()
                .constraintEqualToAnchor_constant(&tile.topAnchor(), 3.0),
            remove
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&tile.trailingAnchor(), -3.0),
        ]);
    }
    Retained::into_super(tile)
}

/// Puts the picture `data` into `view` once the sandboxed decoding service has turned it into pixels, as the
/// previews do, so the app decodes no picture an agent or a pasteboard handed it.
fn show_decoded(mtm: MainThreadMarker, view: &NSImageView, data: &[u8], pictures: Option<&Pictures>) {
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        data.hash(&mut hasher);
        hasher.finish()
    };
    if let Some(image) = pictures.and_then(|p| p.borrow().get(&key).cloned()) {
        view.setImage(Some(&image));
        return;
    }
    let view = dispatch2::MainThreadBound::new(objc2::rc::Weak::new(view), mtm);
    let pictures = dispatch2::MainThreadBound::new(pictures.cloned(), mtm);
    let started = crate::common::util::spawn_with(
        "agent-picture",
        (view, pictures, data.to_vec()),
        move |(view, pictures, data)| {
            let decoded = splat_decoder::decode(&data, splat_decoder::SHOWN_SIDE);
            crate::common::util::main_soon(move || {
                let Some(mtm) = MainThreadMarker::new() else { return };
                let Some(image) = decoded.and_then(splat_decoder::image) else {
                    return;
                };
                if let Some(pictures) = pictures.into_inner(mtm) {
                    pictures.borrow_mut().insert(key, image.clone());
                }
                if let Some(view) = view.into_inner(mtm).load() {
                    view.setImage(Some(&image));
                }
            });
        },
    );
    if let Err((_, why)) = started {
        crate::common::diag::report("showing a picture", why.0);
    }
}

/// The edge insets with the given sides.
pub fn insets(top: f64, left: f64, bottom: f64, right: f64) -> NSEdgeInsets {
    NSEdgeInsets {
        top,
        left,
        bottom,
        right,
    }
}

/// A thin line across a box.
pub fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    line.setTranslatesAutoresizingMaskIntoConstraints(false);
    line
}

/// An SF Symbol at `points`, tinted `color`.
pub fn icon(
    mtm: MainThreadMarker,
    name: &str,
    description: &str,
    points: f64,
    color: &NSColor,
) -> Retained<NSImageView> {
    let view = NSImageView::new(mtm);
    view.setImage(symbol(name, description).as_deref());
    view.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
        points,
        constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    view.setContentTintColor(Some(color));
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    view
}

/// A small bordered push button that sends `action` with `tag` to `target`.
pub(crate) fn button(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
    tag: isize,
) -> Retained<NSButton> {
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, target, action);
    button.setTag(tag);
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Small);
    button.setFont(Some(&NSFont::systemFontOfSize(NSFont::systemFontSizeForControlSize(
        NSControlSize::Small,
    ))));
    button
}

/// A borderless button in the accent colour, read as a link.
pub(crate) fn link(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
    tag: isize,
) -> Retained<NSButton> {
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, target, action);
    button.setTag(tag);
    button.setBordered(false);
    button.setFont(Some(&super::style::small()));
    button.setContentTintColor(Some(&NSColor::linkColor()));
    button
}

/// A borderless symbol button.
pub(crate) fn symbol_button(
    mtm: MainThreadMarker,
    name: &str,
    description: &str,
    points: f64,
    target: &AnyObject,
    action: Sel,
    tag: isize,
) -> Retained<NSButton> {
    let button = crate::common::util::symbol_button(mtm, name, description, points, Some(target), action);
    button.setTag(tag);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    button
}

/// A small spinning progress indicator, already turning.
pub(crate) fn spinner(mtm: MainThreadMarker) -> Retained<NSProgressIndicator> {
    let spinner = NSProgressIndicator::new(mtm);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(NSControlSize::Small);
    spinner.setIndeterminate(true);
    spinner.setDisplayedWhenStopped(false);
    spinner.setTranslatesAutoresizingMaskIntoConstraints(false);
    fixed_size(&spinner, 12.0, 12.0);
    // SAFETY: The indicator is on the main thread, where it animates.
    unsafe { spinner.startAnimation(None) };
    spinner
}

/// Makes `view` one element for VoiceOver, read as `summary`.
pub(crate) fn group(view: &NSView, summary: &str) {
    view.setAccessibilityElement(true);
    view.setAccessibilityRole(Some(constant!(NSAccessibilityGroupRole)));
    view.setAccessibilityLabel(Some(&ns(summary)));
}

/// Fixes the width of `view`.
pub(crate) fn fixed_width(view: &NSView, width: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor().constraintEqualToConstant(width).setActive(true);
}

/// Fixes the height of `view`.
pub(crate) fn fixed_height(view: &NSView, height: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.heightAnchor().constraintEqualToConstant(height).setActive(true);
}

/// Has `view` keep its own width in a row rather than stretch.
pub(crate) fn hug(view: &NSView) {
    view.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    view.setContentCompressionResistancePriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
}

/// Any view as a plain view.
pub(crate) fn view(view: &NSView) -> Retained<NSView> {
    view.retain()
}
