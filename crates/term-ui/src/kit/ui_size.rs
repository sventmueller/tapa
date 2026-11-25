//! The size of the interface's text, which the user chooses in the settings: the sidebar with its symbols and the tab
//! bars take every size from it, text, symbols, rows and the room around them alike. A size in the code is drawn for
//! [`DEFAULT`]; [`scaled`] gives it at the user's size, and the setters here keep a view at it for as long as the view
//! lives, so an open window follows a change at once.

use std::cell::Cell;
use std::ffi::c_void;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSButton, NSControl, NSControlSize, NSFont, NSImageSymbolConfiguration, NSImageView, NSLayoutConstraint,
    NSOutlineView, NSStackView, NSTableView, NSTableViewRowSizeStyle,
};
use objc2_foundation::{NSEdgeInsets, NSIndexSet, NSNotificationCenter, NSObject, NSObjectProtocol, NSRange};

use super::observe::{Observation, observe};
use super::{ns, symbol_at};

/// The size the sizes in the code are drawn for, which shows them as they are.
pub const DEFAULT: f64 = 13.0;
/// The smallest size the settings offer.
pub const MIN: f64 = 10.0;
/// The largest size the settings offer; the native tab bar keeps its height, so a larger title would be cut.
pub const MAX: f64 = 18.0;

/// The notification that tells the views made at the interface size that it changed.
const CHANGED: &str = "TermInterfaceTextSizeChanged";

thread_local! {
    static SIZE: Cell<f64> = const { Cell::new(DEFAULT) };
}

/// The interface's text size; the default until the app sets the user's, as in unit tests.
pub fn current() -> f64 {
    SIZE.with(Cell::get)
}

/// `points`, a size drawn for text of [`DEFAULT`] size, at the interface size the user chose.
pub fn scaled(points: f64) -> f64 {
    points * current() / DEFAULT
}

/// Shows the interface at `size` points, as the settings have it; the app calls it as it starts.
pub fn set(size: f64) {
    let size = valid(size);
    if SIZE.with(|shown| shown.replace(size)) != size {
        // SAFETY: The app's own notification, posted without an object.
        unsafe { NSNotificationCenter::defaultCenter().postNotificationName_object(&ns(CHANGED), None) };
    }
}

/// `size`, or the default where a hand-edited settings file holds something the interface cannot show.
fn valid(size: f64) -> f64 {
    if size.is_finite() {
        size.clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

/// What of a view follows the interface size; each owner keeps one follower per aspect, the last one set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub enum Aspect {
    Font,
    Symbol,
    Rows,
    Indent,
    Spacing,
    Insets,
    /// A length of a layout constraint.
    Length,
    /// The size AppKit draws a control's bezel at.
    ControlSize,
    /// Whatever else a view lays out at the interface size.
    Layout,
}

/// One byte per aspect, whose addresses key the followers a view holds.
static KEYS: [u8; 9] = [0; 9];

impl Aspect {
    fn key(self) -> *const c_void {
        let index = match self {
            Aspect::Font => 0,
            Aspect::Symbol => 1,
            Aspect::Rows => 2,
            Aspect::Indent => 3,
            Aspect::Spacing => 4,
            Aspect::Insets => 5,
            Aspect::Length => 6,
            Aspect::Layout => 7,
            Aspect::ControlSize => 8,
        };
        (&raw const KEYS[index]).cast()
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Follower` does not implement Drop; its observation ends
    // when the follower goes.
    /// The watch that keeps one aspect of a view at the interface size, held by the view so it ends with it.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermInterfaceSizeFollower"]
    #[ivars = Observation]
    struct Follower;

    unsafe impl NSObjectProtocol for Follower {}
);

/// Runs `apply` on `owner` now and again at every change of the interface size while `owner` lives, in place of what
/// `owner` followed for `aspect` before.
pub fn keep<T: Message + 'static>(owner: &T, aspect: Aspect, apply: impl Fn(&T) + 'static) {
    apply(owner);
    let Some(mtm) = MainThreadMarker::new() else { return };
    let weak = Weak::new(owner);
    let observation = observe(mtm, &ns(CHANGED), None, move |_| {
        if let Some(owner) = weak.load() {
            apply(&owner);
        }
    });
    let follower = Follower::alloc(mtm).set_ivars(observation);
    // SAFETY: NSObject's designated initializer, on an allocation that `set_ivars` filled.
    let follower: Retained<Follower> = unsafe { msg_send![super(follower), init] };
    let object: *const T = owner;
    // SAFETY: `owner` is an Objective-C object and `follower` one too; the key is the address of a static, and the
    // retaining policy has the owner keep the follower and release one it replaces.
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            object.cast_mut().cast::<AnyObject>(),
            aspect.key(),
            Retained::as_ptr(&follower).cast_mut().cast::<AnyObject>(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

/// `font` at the interface size, drawn for text of [`DEFAULT`] size.
pub fn font_like(font: &NSFont) -> Retained<NSFont> {
    let descriptor = font.fontDescriptor();
    NSFont::fontWithDescriptor_size(&descriptor, scaled(font.pointSize())).unwrap_or_else(|| font.retain())
}

/// The system font of `points` at the interface size.
pub fn system(points: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize(scaled(points))
}

/// Shows `control`'s text in `base`, a font drawn for [`DEFAULT`], at the interface size, now and after a change.
pub fn set_font(control: &NSControl, base: &NSFont) {
    let base = base.retain();
    keep(control, Aspect::Font, move |control| {
        control.setFont(Some(&font_like(&base)))
    });
}

/// [`set_font`] with the system font of `points`.
pub fn set_system_font(control: &NSControl, points: f64) {
    set_font(control, &NSFont::systemFontOfSize(points));
}

/// A borderless button with the symbol `name` at `points` at the interface size, labelled `description` for VoiceOver
/// and its tooltip, sending `action` to `target`.
pub fn symbol_button(
    mtm: MainThreadMarker,
    name: &str,
    description: &str,
    points: f64,
    target: Option<&AnyObject>,
    action: Sel,
) -> Retained<NSButton> {
    let button = super::symbol_button(mtm, name, description, points, target, action);
    follow_symbol(&button, name, description, points);
    button
}

/// Keeps the symbol `name`, described as `description`, that `button` shows at `points` at the interface size.
pub fn follow_symbol(button: &NSButton, name: &str, description: &str, points: f64) {
    let (name, description) = (name.to_owned(), description.to_owned());
    keep(button, Aspect::Symbol, move |button| {
        let size = scaled(points);
        if let Some(image) = symbol_at(&name, &description, size) {
            button.setImage(Some(&image));
        }
        button.setSymbolConfiguration(Some(&symbol_configuration(size)));
    });
}

/// Keeps the SF Symbol `view` shows at `points` at the interface size.
pub fn follow_image_symbol(view: &NSImageView, points: f64) {
    keep(view, Aspect::Symbol, move |view| {
        view.setSymbolConfiguration(Some(&symbol_configuration(scaled(points))));
    });
}

/// Shows the symbol `name`, described as `description`, in `view` as large as [`super::symbol`] draws it, at the
/// interface size.
pub fn follow_symbol_image(view: &NSImageView, name: &str, description: &str) {
    // An SF Symbol without a configuration has the size of text of the default size.
    let points = DEFAULT;
    let (name, description) = (name.to_owned(), description.to_owned());
    keep(view, Aspect::Symbol, move |view| {
        view.setImage(symbol_at(&name, &description, scaled(points)).as_deref());
    });
}

fn symbol_configuration(points: f64) -> Retained<NSImageSymbolConfiguration> {
    NSImageSymbolConfiguration::configurationWithPointSize_weight(
        points,
        term_render::constant!(objc2_app_kit::NSFontWeightRegular),
    )
}

/// Gives `constraint` the length `base`, drawn for [`DEFAULT`], at the interface size.
pub fn follow_length(constraint: &NSLayoutConstraint, base: f64) {
    keep(constraint, Aspect::Length, move |constraint| {
        constraint.setConstant(scaled(base))
    });
}

/// Gives the rows of `table` the height `base` at the interface size, and builds its rows anew at a change, so cells
/// that take their sizes as they are made show the new ones. A source list no longer sizes its cells' text itself.
pub fn follow_rows(table: &NSTableView, base: f64) {
    table.setRowSizeStyle(NSTableViewRowSizeStyle::Custom);
    let first = Cell::new(true);
    keep(table, Aspect::Rows, move |table| {
        table.setRowHeight(scaled(base));
        if !first.replace(false) {
            table.reloadData();
        }
    });
}

/// [`follow_rows`] with the height `table` has now, AppKit's own where the code set none.
pub fn follow_row_height(table: &NSTableView) {
    follow_rows(table, table.rowHeight());
}

/// Builds the rows of `table`, whose cells take their sizes as they are made and whose delegate gives each row its
/// height, anew at a change of the interface size.
pub fn follow_row_heights(table: &NSTableView) {
    let first = Cell::new(true);
    keep(table, Aspect::Rows, move |table| {
        if !first.replace(false) {
            table.reloadData();
            let rows = usize::try_from(table.numberOfRows()).unwrap_or(0);
            if rows > 0 {
                let all = NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, rows));
                table.noteHeightOfRowsWithIndexesChanged(&all);
            }
        }
    });
}

/// Indents each level of `outline` by `base` at the interface size.
pub fn follow_indent(outline: &NSOutlineView, base: f64) {
    keep(outline, Aspect::Indent, move |outline| {
        outline.setIndentationPerLevel(scaled(base))
    });
}

/// Spaces the views of `stack` by `base` at the interface size.
pub fn follow_spacing(stack: &NSStackView, base: f64) {
    keep(stack, Aspect::Spacing, move |stack| stack.setSpacing(scaled(base)));
}

/// Keeps `base` around the views of `stack`, at the interface size.
pub fn follow_insets(stack: &NSStackView, base: NSEdgeInsets) {
    keep(stack, Aspect::Insets, move |stack| {
        stack.setEdgeInsets(NSEdgeInsets {
            top: scaled(base.top),
            left: scaled(base.left),
            bottom: scaled(base.bottom),
            right: scaled(base.right),
        });
    });
}

/// AppKit's control sizes with the height of a pop-up button's bezel at each.
const CONTROL_HEIGHTS: [(NSControlSize, f64); 4] = [
    (NSControlSize::Mini, 16.0),
    (NSControlSize::Small, 20.0),
    (NSControlSize::Regular, 24.0),
    (NSControlSize::Large, 28.0),
];

/// The control size whose bezel comes nearest to that of `base` at the interface size.
pub fn control_size(base: NSControlSize) -> NSControlSize {
    let Some(&(_, height)) = CONTROL_HEIGHTS.iter().find(|(known, _)| *known == base) else {
        return base;
    };
    let wanted = scaled(height);
    CONTROL_HEIGHTS
        .into_iter()
        .min_by(|a, b| (a.1 - wanted).abs().total_cmp(&(b.1 - wanted).abs()))
        .map_or(base, |(size, _)| size)
}

/// Keeps `control`, drawn at the control size `base` at the default interface size, at the control size nearest to it
/// at the user's; AppKit draws a bezel only in these steps, while the text inside follows the size smoothly.
pub fn follow_control_size(control: &NSControl, base: NSControlSize) {
    keep(control, Aspect::ControlSize, move |control| {
        control.setControlSize(control_size(base));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_outside_the_settings_range_is_brought_into_it() {
        assert_eq!(valid(14.0), 14.0);
        assert_eq!(valid(2.0), MIN);
        assert_eq!(valid(400.0), MAX);
        assert_eq!(valid(f64::NAN), DEFAULT);
    }

    #[test]
    fn sizes_scale_by_the_share_of_the_default() {
        assert_eq!(scaled(22.0), 22.0, "the default shows the code's sizes");
        SIZE.with(|size| size.set(DEFAULT * 1.5));
        assert_eq!(scaled(22.0), 33.0);
        SIZE.with(|size| size.set(DEFAULT));
    }

    #[test]
    fn a_control_steps_through_the_control_sizes_with_the_interface_size() {
        for (size, _) in CONTROL_HEIGHTS {
            assert_eq!(control_size(size), size, "the default keeps every control as it is");
        }
        SIZE.with(|size| size.set(MAX));
        assert_eq!(control_size(NSControlSize::Small), NSControlSize::Large);
        assert_eq!(control_size(NSControlSize::Regular), NSControlSize::Large);
        SIZE.with(|size| size.set(MIN));
        assert_eq!(control_size(NSControlSize::Small), NSControlSize::Mini);
        SIZE.with(|size| size.set(DEFAULT));
    }

    #[test]
    fn every_aspect_has_its_own_key() {
        let all = [
            Aspect::Font,
            Aspect::Symbol,
            Aspect::Rows,
            Aspect::Indent,
            Aspect::Spacing,
            Aspect::Insets,
            Aspect::Length,
            Aspect::Layout,
            Aspect::ControlSize,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(|aspect| aspect.key() as usize).collect();
        assert_eq!(keys.len(), all.len());
    }
}
