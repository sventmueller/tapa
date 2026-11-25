//! The heading of the window as a toolbar item at the content's leading edge, in place of the title macOS puts further
//! in, so that it lines up with the tab bar and the content: in bold the folder of what the tab in front shows, below
//! it the branch, for every kind of tab. macOS keeps the window's own title for the Window menu and Mission Control.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSColor, NSFont, NSFontWeightSemibold, NSLayoutAttribute, NSLineBreakMode, NSStackView, NSTextField, NSToolbarItem,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow,
};
use objc2_foundation::NSString;
use term_ui::kit::{label, ns, ui_size};

/// How far the title keeps from where the toolbar puts its first item.
const LEAD: f64 = 4.0;

/// How many points the heading's text grows by at most with the interface size.
const MAX_GROWTH: f64 = 2.0;

/// `points` at the interface size, but no larger than the toolbar's fixed height leaves room for two lines.
fn capped(points: f64) -> f64 {
    ui_size::scaled(points).min(points + MAX_GROWTH)
}

/// The item's identifier.
pub(super) const ID: &str = "term.title";

/// The item, empty until [`show`] fills it.
pub(super) fn item(mtm: MainThreadMarker, identifier: &NSString) -> Retained<NSToolbarItem> {
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), identifier);
    // SAFETY: AppKit sets its exported font weight constants when it loads and never changes them.
    let weight = unsafe { NSFontWeightSemibold };
    let title = label(mtm, "");
    ui_size::keep(&*title, ui_size::Aspect::Font, move |title| {
        let size = capped(NSFont::systemFontSize());
        title.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    });
    let subtitle = label(mtm, "");
    ui_size::keep(&*subtitle, ui_size::Aspect::Font, |subtitle| {
        subtitle.setFont(Some(&NSFont::systemFontOfSize(capped(NSFont::smallSystemFontSize()))));
    });
    subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
    for field in [&title, &subtitle] {
        field.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    }
    let stack = NSStackView::stackViewWithViews(
        &objc2_foundation::NSArray::from_slice(&[&*title as &NSView, &*subtitle]),
        mtm,
    );
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setSpacing(0.0);
    // The toolbar starts its first item this far short of the column the tabs and the content's heading share.
    stack.setEdgeInsets(objc2_foundation::NSEdgeInsets {
        top: 0.0,
        left: LEAD,
        bottom: 0.0,
        right: 0.0,
    });
    item.setView(Some(&stack));
    item.setBordered(false);
    item.setLabel(&ns(""));
    item
}

/// Shows `heading`, the folder of what the tab in front shows, in bold and `detail`, its branch, below it in
/// `ns_window`'s item; the same for every kind of tab.
pub(crate) fn show(ns_window: &NSWindow, heading: &str, detail: &str) {
    let Some([title, subtitle]) = fields(ns_window) else {
        return;
    };
    let heading = ns(heading);
    if title.stringValue() != heading {
        title.setStringValue(&heading);
        title.setToolTip(Some(&heading));
    }
    set_detail(&subtitle, detail);
}

/// Shows `text` for a while in place of the branch below the heading.
pub(crate) fn notice(ns_window: &NSWindow, text: &str) {
    if let Some([_, subtitle]) = fields(ns_window) {
        set_detail(&subtitle, text);
    }
}

fn set_detail(subtitle: &NSTextField, text: &str) {
    let text = ns(text);
    if subtitle.stringValue() != text {
        subtitle.setStringValue(&text);
    }
    subtitle.setHidden(text.length() == 0);
}

/// The item's two fields, the heading's and the branch's.
fn fields(ns_window: &NSWindow) -> Option<[Retained<NSTextField>; 2]> {
    let toolbar = ns_window.toolbar()?;
    let id = ns(ID);
    let item = toolbar.items().iter().find(|item| *item.itemIdentifier() == *id)?;
    let stack = item.view()?.downcast::<NSStackView>().ok()?;
    let fields: Vec<Retained<NSTextField>> = stack
        .arrangedSubviews()
        .iter()
        .filter_map(|view| view.downcast::<NSTextField>().ok())
        .collect();
    fields.try_into().ok()
}
