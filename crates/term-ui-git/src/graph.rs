//! The commit graph beside the history, as VS Code draws it: every commit a dot in its lane, lines from each commit
//! down to its parents, a lane's colour kept for as long as the lane runs.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSBackgroundStyle, NSBezierPath, NSColor, NSTableCellView, NSTableRowView,
    NSTableViewSelectionHighlightStyle, NSUserInterfaceLayoutDirection, NSView,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

use term_ui::kit::guarded;
use term_ui::kit::ui_size::scaled;

/// The width of one lane.
const LANE: f64 = 12.0;
const DOT_RADIUS: f64 = 3.5;
const LINE_WIDTH: f64 = 1.5;

pub(crate) use term_git::panel::GraphRow;

/// The colour of lane colour `index`: the accent colour first, then system colours that keep their contrast in
/// light and dark mode.
fn lane_color(index: usize) -> Retained<NSColor> {
    match index % 8 {
        0 => NSColor::controlAccentColor(),
        1 => NSColor::systemPurpleColor(),
        2 => NSColor::systemOrangeColor(),
        3 => NSColor::systemGreenColor(),
        4 => NSColor::systemPinkColor(),
        5 => NSColor::systemTealColor(),
        6 => NSColor::systemIndigoColor(),
        _ => NSColor::systemBrownColor(),
    }
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `GraphView` does not implement Drop.
    /// One row's part of the graph, as tall as the row so the lines meet those of the rows around it.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitGraphView"]
    #[ivars = RefCell<GraphRow>]
    pub(crate) struct GraphView;

    unsafe impl NSObjectProtocol for GraphView {}

    impl GraphView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }
    }
);

impl GraphView {
    pub(crate) fn new(mtm: MainThreadMarker, row: GraphRow) -> Retained<GraphView> {
        let this = Self::alloc(mtm).set_ivars(RefCell::new(row));
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let view: Retained<GraphView> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        view.setAccessibilityElement(false);
        view
    }

    /// The width the row's lanes take.
    pub(crate) fn width(row: &GraphRow) -> f64 {
        row.width.max(1) as f64 * lane()
    }

    fn draw(&self) {
        let row = self.ivars().borrow();
        let bounds = self.bounds();
        let height = bounds.size.height;
        let middle = height / 2.0;
        let (lane_width, radius, stroke) = (lane(), scaled(DOT_RADIUS), scaled(LINE_WIDTH));
        let rtl = self.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        let x = |lane: usize| {
            let along = lane as f64 * lane_width + lane_width / 2.0;
            if rtl { bounds.size.width - along } else { along }
        };
        let gap = |lane: usize| if lane == row.lane { radius + scaled(1.0) } else { 0.0 };
        for segment in &row.top {
            line(
                NSPoint::new(x(segment.from), 0.0),
                NSPoint::new(x(segment.to), middle - gap(segment.to)),
                segment.color,
                stroke,
            );
        }
        for segment in &row.bottom {
            line(
                NSPoint::new(x(segment.from), middle + gap(segment.from)),
                NSPoint::new(x(segment.to), height),
                segment.color,
                stroke,
            );
        }
        if row.lane == usize::MAX {
            return;
        }
        let color = lane_color(row.color);
        let center = NSPoint::new(x(row.lane), middle);
        let dot = |radius: f64| {
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                NSPoint::new(center.x - radius, center.y - radius),
                NSSize::new(radius * 2.0, radius * 2.0),
            ))
        };
        if row.head {
            let ring = dot(radius);
            ring.setLineWidth(stroke + scaled(0.5));
            color.setStroke();
            ring.stroke();
        } else if row.merge {
            let ring = dot(radius);
            ring.setLineWidth(stroke);
            color.setStroke();
            ring.stroke();
            color.setFill();
            dot(radius - scaled(2.0)).fill();
        } else {
            color.setFill();
            dot(radius).fill();
        }
    }
}

/// The width of one lane at the interface size.
pub(crate) fn lane() -> f64 {
    scaled(LANE)
}

/// How far a selection keeps from the list's edges, and how round its corners are, as the source list draws it.
const SELECTION_INSET: f64 = 10.0;
const SELECTION_RADIUS: f64 = 5.0;
/// How far a selection reaches past a cell that begins closer to the edge than `SELECTION_INSET`.
const SELECTION_MARGIN: f64 = 4.0;

define_class!(
    // SAFETY: NSTableRowView has no subclassing requirements and `GraphRowView` does not implement Drop.
    /// A row of the Git list whose selection begins after the graph's lanes, leaving the dot and the lines visible, and
    /// before a cell set closer to the edge than the source list's inset, leaving its icon whole.
    #[unsafe(super(NSTableRowView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitGraphRowView"]
    pub(crate) struct GraphRowView;

    unsafe impl NSObjectProtocol for GraphRowView {}

    impl GraphRowView {
        /// The outline view sets its own style on every row view it adds; AppKit draws a source list's selection
        /// across the row by itself, so the row takes none and draws its own in `drawBackgroundInRect:`.
        #[unsafe(method(setSelectionHighlightStyle:))]
        fn set_selection_highlight_style(&self, _style: NSTableViewSelectionHighlightStyle) {
            // SAFETY: The superclass's own setter, with its signature.
            let _: () = unsafe { msg_send![super(self), setSelectionHighlightStyle: NSTableViewSelectionHighlightStyle::None] };
        }

        #[unsafe(method(setSelected:))]
        fn set_selected(&self, selected: bool) {
            // SAFETY: The superclass's own setter, with its signature.
            let _: () = unsafe { msg_send![super(self), setSelected: selected] };
            guarded((), || self.restyle());
        }

        #[unsafe(method(setEmphasized:))]
        fn set_emphasized(&self, emphasized: bool) {
            // SAFETY: The superclass's own setter, with its signature.
            let _: () = unsafe { msg_send![super(self), setEmphasized: emphasized] };
            guarded((), || self.restyle());
        }

        #[unsafe(method(drawBackgroundInRect:))]
        fn draw_background(&self, dirty: NSRect) {
            guarded((), || {
                // SAFETY: The superclass's own method, with its signature.
                let _: () = unsafe { msg_send![super(self), drawBackgroundInRect: dirty] };
                let Some(rect) = self.selection_rect().filter(|_| self.isSelected()) else {
                    return;
                };
                let color = if self.isEmphasized() {
                    NSColor::selectedContentBackgroundColor()
                } else {
                    NSColor::unemphasizedSelectedContentBackgroundColor()
                };
                color.setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, SELECTION_RADIUS, SELECTION_RADIUS).fill();
            });
        }
    }
);

impl GraphRowView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<GraphRowView> {
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![GraphRowView::alloc(mtm), initWithFrame: NSRect::ZERO] }
    }

    /// Redraws the selection and hands the cells the background they stand on, which AppKit does only for a row with
    /// a selection style of its own.
    fn restyle(&self) {
        self.setNeedsDisplay(true);
        let style = if self.isSelected() && self.isEmphasized() {
            NSBackgroundStyle::Emphasized
        } else {
            NSBackgroundStyle::Normal
        };
        for view in self.subviews().iter() {
            if let Some(cell) = view.downcast_ref::<NSTableCellView>() {
                cell.setBackgroundStyle(style);
            }
        }
    }

    /// The selection from the end of the row's lanes, or in a row without them from the leading inset or just before
    /// its cell, whichever is nearer the edge, to the trailing inset.
    fn selection_rect(&self) -> Option<NSRect> {
        let bounds = self.bounds();
        let (mut start, mut end) = (
            bounds.origin.x + SELECTION_INSET,
            bounds.origin.x + bounds.size.width - SELECTION_INSET,
        );
        let rtl = self.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        for cell in self
            .subviews()
            .iter()
            .filter(|v| v.isKindOfClass(NSTableCellView::class()))
        {
            let frame = cell.frame();
            if rtl {
                end = end.max(frame.origin.x + frame.size.width + SELECTION_MARGIN);
            } else {
                start = start.min(frame.origin.x - SELECTION_MARGIN);
            }
        }
        let (start, end) = (start.max(bounds.origin.x), end.min(bounds.origin.x + bounds.size.width));
        // The dot's lane ends a little past the dot, so the selection may start just inside the lanes.
        let (left, right) = match graph_in(self).map(|g| self.convertRect_fromView(g.bounds(), Some(&g))) {
            Some(lanes) if rtl => (start, lanes.origin.x + 1.0),
            Some(lanes) => (lanes.origin.x + lanes.size.width - 1.0, end),
            None => (start, end),
        };
        (right > left).then(|| {
            NSRect::new(
                NSPoint::new(left, bounds.origin.y),
                NSSize::new(right - left, bounds.size.height),
            )
        })
    }
}

/// The graph somewhere inside `view`.
fn graph_in(view: &NSView) -> Option<Retained<GraphView>> {
    view.subviews()
        .iter()
        .find_map(|child| match child.clone().downcast::<GraphView>() {
            Ok(graph) => Some(graph),
            Err(_) => graph_in(&child),
        })
}

/// A line `width` wide from `start` to `end`, straight down within a lane or bending over to another one.
fn line(start: NSPoint, end: NSPoint, color: usize, width: f64) {
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(width);
    path.moveToPoint(start);
    if (start.x - end.x).abs() < f64::EPSILON {
        path.lineToPoint(end);
    } else {
        let bend = (start.y + end.y) / 2.0;
        path.curveToPoint_controlPoint1_controlPoint2(end, NSPoint::new(start.x, bend), NSPoint::new(end.x, bend));
    }
    lane_color(color).setStroke();
    path.stroke();
}
