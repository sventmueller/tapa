//! The lists of the sidebar's panels and the page around them: a source-list table or outline of one
//! column in a scroll view, the page's margins, and rows of a symbol and a name that VoiceOver reads as one.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSFont, NSImageView, NSLineBreakMode, NSOutlineView, NSScrollView, NSStackView,
    NSStackViewGravity, NSTableCellView, NSTableColumn, NSTableColumnResizingOptions, NSTableView,
    NSTableViewColumnAutoresizingStyle, NSTableViewStyle, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize};

use crate::kit::style::space;
use crate::kit::{activate, label, ns};

/// The frame a panel's list starts with, before the sidebar lays it out.
pub const LIST_FRAME: NSRect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(240.0, 400.0));
/// How much further in a row sits than its parent, in every panel's list.
pub(crate) const INDENT: f64 = 12.0;
/// How far a top-level row built as a stack keeps from the list's edges.
pub(crate) const ROW_INSET: f64 = 4.0;
/// How far text and controls of a panel keep from the sidebar's edges: as far as the panel's title.
pub(crate) const MARGIN: f64 = 8.0;
/// How far AppKit's source list sets its disclosure triangles in past the panel's title, for a list that spans the
/// panel's width.
pub(crate) const OUTDENT: f64 = 5.5;
/// The margins of a panel's page; its list spans the width, so its triangles line up with the panel's title.
pub(crate) const PAGE_INSETS: NSEdgeInsets = NSEdgeInsets {
    top: 4.0,
    left: 0.0,
    bottom: 6.0,
    right: 0.0,
};

/// Makes `view`, an outline view made with [`LIST_FRAME`] or another frame, a source list of one column `id`
/// without a header, named `name` for VoiceOver, whose one column takes the width; the scroll view that shows it.
pub fn source_list_outline(
    mtm: MainThreadMarker,
    view: &NSOutlineView,
    id: &str,
    name: &str,
) -> Retained<NSScrollView> {
    let column = one_column(mtm, view, id, name);
    // SAFETY: The column was just added to this outline view, which keeps it.
    unsafe { view.setOutlineTableColumn(Some(&column)) };
    crate::kit::ui_size::follow_indent(view, INDENT);
    crate::kit::ui_size::follow_row_height(view);
    view.setAutoresizesOutlineColumn(false);
    view.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle);
    let scroll = scroll(mtm, view);
    view.sizeLastColumnToFit();
    scroll
}

fn one_column(mtm: MainThreadMarker, table: &NSTableView, id: &str, name: &str) -> Retained<NSTableColumn> {
    let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(id));
    column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
    // The table scales its columns by their share of its width, so the one column starts as wide as the table.
    column.setWidth(table.frame().size.width);
    table.addTableColumn(&column);
    table.setHeaderView(None);
    table.setStyle(NSTableViewStyle::SourceList);
    table.setAccessibilityLabel(Some(&ns(name)));
    column
}

fn scroll(mtm: MainThreadMarker, list: &NSView) -> Retained<NSScrollView> {
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), list.frame());
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(list));
    scroll
}

/// `view` in a row that keeps [`MARGIN`] from the sidebar's edges, for a panel's page beside its list.
pub fn padded(mtm: MainThreadMarker, view: &NSView) -> Retained<NSStackView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: MARGIN,
        bottom: 0.0,
        right: MARGIN,
    });
    row.addArrangedSubview(view);
    row
}

/// A heading of the sidebar, `text` cut at its end: a panel's title, or a section's title in a panel's list.
pub fn heading(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let heading = label(mtm, text);
    crate::kit::ui_size::set_font(&heading, &NSFont::boldSystemFontOfSize(crate::kit::style::SMALL));
    heading.setTextColor(Some(&NSColor::secondaryLabelColor()));
    heading.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    heading.setContentCompressionResistancePriority_forOrientation(
        200.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    heading
}

/// The cell of a section's row in a panel's list: its [`heading`] `title`, level with the row's disclosure triangle.
pub fn section_cell(mtm: MainThreadMarker, title: &str) -> Retained<NSTableCellView> {
    let cell = NSTableCellView::new(mtm);
    let text = heading(mtm, title);
    cell.addSubview(&text);
    text.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        text.leadingAnchor()
            .constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0),
        text.trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -4.0),
        text.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]);
    // Not the cell's text field: the source list would draw it dimmer than the panel's title.
    cell
}

/// A panel's page: a vertical stack with the margins of [`PAGE_INSETS`].
pub fn panel_page(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let page = NSStackView::new(mtm);
    page.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    crate::kit::ui_size::follow_insets(&page, PAGE_INSETS);
    page
}

/// A row of a panel's list, `level` deep: the symbol `name`, described as `description` and tinted `tint`, then
/// `text` cut at its end; VoiceOver reads the row as one element named `spoken`. Views added in the trailing gravity
/// go after the text.
pub fn icon_row(
    mtm: MainThreadMarker,
    (name, description, tint): (&str, &str, &NSColor),
    text: &str,
    level: usize,
    spoken: &str,
) -> (Retained<NSStackView>, Retained<NSTextField>) {
    let icon = NSImageView::new(mtm);
    crate::kit::ui_size::follow_symbol_image(&icon, name, description);
    icon.setContentTintColor(Some(tint));
    let text = label(mtm, text);
    crate::kit::ui_size::set_system_font(&text, crate::kit::style::BODY);
    text.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    crate::kit::ui_size::follow_spacing(&row, space::S);
    crate::kit::ui_size::keep(&*row, crate::kit::ui_size::Aspect::Insets, move |row| {
        row.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: row_indent(level),
            bottom: 0.0,
            right: ROW_INSET,
        });
    });
    row.addView_inGravity(&icon, NSStackViewGravity::Leading);
    row.addView_inGravity(&text, NSStackViewGravity::Leading);
    row.setAccessibilityElement(true);
    row.setAccessibilityRole(Some(term_render::constant!(objc2_app_kit::NSAccessibilityGroupRole)));
    row.setAccessibilityLabel(Some(&ns(spoken)));
    (row, text)
}

/// Where a row `level` deep begins.
#[expect(clippy::cast_precision_loss, reason = "a nesting level of a few at most")]
fn row_indent(level: usize) -> f64 {
    ROW_INSET + level as f64 * crate::kit::ui_size::scaled(INDENT)
}
