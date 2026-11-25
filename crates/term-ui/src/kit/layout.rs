use objc2::rc::Retained;
use objc2_app_kit::{NSLayoutConstraint, NSView};
use objc2_foundation::NSEdgeInsets;

/// No room between a view and its container's edges.
const FLUSH: NSEdgeInsets = NSEdgeInsets {
    top: 0.0,
    left: 0.0,
    bottom: 0.0,
    right: 0.0,
};

/// Turns on each of `constraints`, in their order.
pub fn activate(constraints: impl IntoIterator<Item = Retained<NSLayoutConstraint>>) {
    for constraint in constraints {
        constraint.setActive(true);
    }
}

/// Has Auto Layout place each of `views` by its constraints instead of the frames its autoresizing mask would give.
pub fn auto_layout(views: &[&NSView]) {
    for view in views {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
    }
}

/// Pins `view` to all four edges of `container`, which must be its superview.
pub fn pin_edges(view: &NSView, container: &NSView) {
    pin_inset(view, container, FLUSH);
}

/// Pins `view` to the four edges of `container`, `insets` in from each; `left` and `right` are the leading and the
/// trailing edge, which swap in a right-to-left interface.
pub fn pin_inset(view: &NSView, container: &NSView, insets: NSEdgeInsets) -> [Retained<NSLayoutConstraint>; 4] {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let constraints = [
        view.leadingAnchor()
            .constraintEqualToAnchor_constant(&container.leadingAnchor(), insets.left),
        view.trailingAnchor()
            .constraintEqualToAnchor_constant(&container.trailingAnchor(), -insets.right),
        view.topAnchor()
            .constraintEqualToAnchor_constant(&container.topAnchor(), insets.top),
        view.bottomAnchor()
            .constraintEqualToAnchor_constant(&container.bottomAnchor(), -insets.bottom),
    ];
    activate(constraints.clone());
    constraints
}

/// Pins the leading and the trailing edge of `view` to those of `other`, `inset` in from each.
pub fn pin_sides(view: &NSView, other: &NSView, inset: f64) -> [Retained<NSLayoutConstraint>; 2] {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let constraints = [
        view.leadingAnchor()
            .constraintEqualToAnchor_constant(&other.leadingAnchor(), inset),
        view.trailingAnchor()
            .constraintEqualToAnchor_constant(&other.trailingAnchor(), -inset),
    ];
    activate(constraints.clone());
    constraints
}

/// Centres `view` on `other`, across and up and down.
pub fn center_in(view: &NSView, other: &NSView) -> [Retained<NSLayoutConstraint>; 2] {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let constraints = [
        view.centerXAnchor().constraintEqualToAnchor(&other.centerXAnchor()),
        view.centerYAnchor().constraintEqualToAnchor(&other.centerYAnchor()),
    ];
    activate(constraints.clone());
    constraints
}

/// Centres `view` up and down on `other`.
pub fn center_y(view: &NSView, other: &NSView) -> Retained<NSLayoutConstraint> {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    let constraint = view.centerYAnchor().constraintEqualToAnchor(&other.centerYAnchor());
    constraint.setActive(true);
    constraint
}

/// Makes `view` exactly `width` by `height` points.
pub fn fixed_size(view: &NSView, width: f64, height: f64) -> [Retained<NSLayoutConstraint>; 2] {
    let constraints = [
        view.widthAnchor().constraintEqualToConstant(width),
        view.heightAnchor().constraintEqualToConstant(height),
    ];
    activate(constraints.clone());
    constraints
}
