//! Line numbers beside the text, drawn for the lines on screen only. A view of its own beside the scroll view:
//! NSScrollView's ruler stops TextKit 2 text from drawing on macOS 27. It is the document of a scroll view of its own
//! that follows the editor's, so scrolling moves what it drew and only the lines coming into view are drawn.

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSEvent, NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSScrollElasticity, NSScrollView,
    NSStringDrawing, NSTextLayoutFragment, NSUserInterfaceLayoutDirection, NSView, NSViewFrameDidChangeNotification,
};
use objc2_foundation::{NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

use super::view::EditorTextView;
use crate::util::{constant, guarded, ns};

/// Where the numbers begin, level with the left edge of the tab bar.
const NUMBERS_LEFT: f64 = 6.0;

/// Between the numbers and the strip of fold triangles.
const NUMBERS_GAP: f64 = 3.0;

/// The strip at the column's right edge that shows and answers change marks.
const CHANGE_STRIP: f64 = 8.0;

/// The width of the strip of fold triangles, between the numbers and the change marks.
const FOLD_WIDTH: f64 = 12.0;

/// The size of the numbers for an editor whose text has `font_size`.
fn number_font(font_size: f64) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(font_size * 0.85, constant!(objc2_app_kit::NSFontWeightRegular))
}

fn number_attributes(font_size: f64) -> Retained<NSDictionary<NSString, objc2::runtime::AnyObject>> {
    NSDictionary::<NSString, objc2::runtime::AnyObject>::from_retained_objects(
        &[
            constant!(NSFontAttributeName),
            constant!(NSForegroundColorAttributeName),
        ],
        &[number_font(font_size).into(), NSColor::tertiaryLabelColor().into()],
    )
}

/// The column's width for a text of `lines` lines in `font_size`: as wide as its highest number needs.
pub(crate) fn width_for(lines: usize, font_size: f64) -> f64 {
    let digits = lines.max(1).to_string();
    let widest = ns(&"0".repeat(digits.len()));
    // SAFETY: The attributes hold a font and a colour, as the method expects.
    let numbers = unsafe { widest.sizeWithAttributes(Some(&number_attributes(font_size))) }.width;
    (NUMBERS_LEFT + numbers + NUMBERS_GAP + FOLD_WIDTH + CHANGE_STRIP).ceil()
}

/// What the numbers are drawn with in one size of the text: their attributes and the width of each digit, so a row
/// measures its number without asking AppKit.
struct Looks {
    font_size: u64,
    normal: Retained<NSDictionary<NSString, objc2::runtime::AnyObject>>,
    digits: [f64; 10],
}

impl Looks {
    fn new(font_size: f64) -> Looks {
        let normal = number_attributes(font_size);
        let digits = std::array::from_fn(|digit| {
            // SAFETY: The attributes hold a font and a colour, as the method expects.
            unsafe { ns(&digit.to_string()).sizeWithAttributes(Some(&normal)) }.width
        });
        Looks {
            font_size: font_size.to_bits(),
            normal,
            digits,
        }
    }

    fn width(&self, number: usize) -> f64 {
        number_width(number, &self.digits)
    }
}

/// The width of `number` written out in digits of the widths `digits`.
fn number_width(number: usize, digits: &[f64; 10]) -> f64 {
    let mut rest = number;
    let mut width = 0.0;
    loop {
        width += digits[rest % 10];
        rest /= 10;
        if rest == 0 {
            return width;
        }
    }
}

/// A tooltip of the column: where, and what it says.
type Tip = (NSRect, String);

pub(crate) struct GutterIvars {
    editor: Weak<EditorTextView>,
    /// The texts of the markers' tooltips; a view holds the owners of its tooltips only weakly.
    tips: std::cell::RefCell<Vec<Retained<NSString>>>,
    /// The tooltips offered, which are offered again only when they change.
    offered: std::cell::RefCell<Vec<Tip>>,
    looks: std::cell::RefCell<Option<std::rc::Rc<Looks>>>,
    /// Whether the mouse is over the column, which then shows every fold's triangle.
    hovered: std::cell::Cell<bool>,
    /// The constraint that holds the column's width, which follows the number of lines.
    width: std::cell::RefCell<Option<Retained<objc2_app_kit::NSLayoutConstraint>>>,
    /// The resizing of the text it follows, which ends with the column.
    watches: std::cell::RefCell<Vec<crate::util::Observation>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `LineNumbers` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermEditorLineNumbers"]
    #[ivars = GutterIvars]
    pub(crate) struct LineNumbers;

    unsafe impl NSObjectProtocol for LineNumbers {}

    impl LineNumbers {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            guarded((), || {
                if let Some(scroll) = self.ivars().editor.load().and_then(|editor| editor.enclosingScrollView()) {
                    scroll.scrollWheel(event);
                }
            });
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, rect: NSRect) {
            guarded((), || self.draw_numbers(rect));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            guarded((), || self.clicked(event));
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            guarded((), || self.set_hovered(true));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            guarded((), || self.set_hovered(false));
        }

        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
            guarded((), || self.track_hover());
        }
    }
);

impl LineNumbers {
    /// A click on the change strip opens a change mark's committed lines, one on the fold strip folds or unfolds.
    fn clicked(&self, event: &NSEvent) {
        let x = self.convertPoint_fromView(event.locationInWindow(), None).x;
        let fold = self.fold_left();
        if x >= self.bounds().size.width - CHANGE_STRIP {
            let _ = self.click_change(event);
        } else if (fold - 3.0..fold + FOLD_WIDTH).contains(&x) {
            self.fold_click(event);
        }
    }

    /// Where the strip of fold triangles begins, right of the numbers.
    fn fold_left(&self) -> f64 {
        self.bounds().size.width - CHANGE_STRIP - FOLD_WIDTH
    }

    /// Holds the column at `constraint`'s width, which `fit` sets.
    pub(crate) fn set_width_constraint(&self, constraint: Retained<objc2_app_kit::NSLayoutConstraint>) {
        *self.ivars().width.borrow_mut() = Some(constraint);
        self.fit();
    }

    /// Makes the column as wide as the editor's highest line number needs.
    pub(crate) fn fit(&self) {
        let Some(editor) = self.ivars().editor.load() else {
            return;
        };
        let width = width_for(editor.document().text().line_count(), editor.ivars().font_size.get());
        if let Some(constraint) = self.ivars().width.borrow().as_ref()
            && (constraint.constant() - width).abs() > 0.5
        {
            constraint.setConstant(width);
            self.redraw();
        }
        self.size_to(&editor);
        self.follow(&editor);
    }

    /// Marks all the column drew for drawing again, as a change of what it shows beside lines asks.
    pub(crate) fn redraw(&self) {
        self.setNeedsDisplay(true);
    }

    /// Gives the column the width its constraint holds and the height of `editor`'s text.
    fn size_to(&self, editor: &EditorTextView) {
        let width = self.ivars().width.borrow().as_ref().map_or(0.0, |c| c.constant());
        let size = NSSize::new(width, editor.frame().size.height);
        if self.frame().size != size {
            self.setFrameSize(size);
        }
    }

    /// Scrolls the column to where `editor` is scrolled, in the same pass, so the numbers never lag the text.
    pub(crate) fn follow(&self, editor: &EditorTextView) {
        let (Some(theirs), Some(ours)) = (editor.enclosingScrollView(), self.enclosingScrollView()) else {
            return;
        };
        let y = theirs.contentView().bounds().origin.y;
        let clip = ours.contentView();
        let origin = clip.bounds().origin;
        if origin.y.to_bits() != y.to_bits() || origin.x != 0.0 {
            clip.setBoundsOrigin(NSPoint::new(0.0, y));
        }
    }

    /// The text changed size: the column takes its height, and what it drew off screen may lie elsewhere now; what is
    /// on screen `viewport` redraws when a line on it moved.
    fn editor_resized(&self) {
        let Some(editor) = self.ivars().editor.load() else {
            return;
        };
        let before = self.frame().size.height;
        self.size_to(&editor);
        self.follow(&editor);
        if (self.frame().size.height - before).abs() < 0.5 {
            return;
        }
        let (bounds, visible) = (self.bounds(), self.visibleRect());
        let bottom = visible.origin.y + visible.size.height;
        let width = bounds.size.width;
        if visible.origin.y > 0.0 {
            self.setNeedsDisplayInRect(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width, visible.origin.y),
            ));
        }
        if bounds.size.height > bottom {
            let below = NSSize::new(width, bounds.size.height - bottom);
            self.setNeedsDisplayInRect(NSRect::new(NSPoint::new(0.0, bottom), below));
        }
    }

    /// Draws the triangle of a fold that starts in `row`: pointing right when folded, always shown; pointing down when
    /// open, shown while the mouse is over the column.
    fn fold_mark(&self, row: NSRect, folded: bool) {
        if !folded && !self.ivars().hovered.get() {
            return;
        }
        let side = 7.0;
        let x = row.origin.x + (row.size.width - side) / 2.0;
        let y = row.origin.y + (row.size.height - side) / 2.0;
        let path = objc2_app_kit::NSBezierPath::bezierPath();
        if folded {
            path.moveToPoint(NSPoint::new(x + 1.0, y));
            path.lineToPoint(NSPoint::new(x + side, y + side / 2.0));
            path.lineToPoint(NSPoint::new(x + 1.0, y + side));
        } else {
            path.moveToPoint(NSPoint::new(x, y + 1.0));
            path.lineToPoint(NSPoint::new(x + side, y + 1.0));
            path.lineToPoint(NSPoint::new(x + side / 2.0, y + side));
        }
        path.closePath();
        NSColor::secondaryLabelColor().setFill();
        path.fill();
    }

    /// Folds or unfolds the range that starts on the clicked line.
    fn fold_click(&self, event: &NSEvent) {
        let Some(editor) = self.ivars().editor.load() else {
            return;
        };
        let in_editor = editor.convertPoint_fromView(event.locationInWindow(), None);
        if let Some(line) = super::highlight::line_at(&editor, NSPoint::new(0.0, in_editor.y)) {
            super::fold::toggle(&editor, line);
        }
    }

    fn set_hovered(&self, hovered: bool) {
        self.ivars().hovered.set(hovered);
        self.redraw();
    }

    /// Hears the mouse enter and leave the column.
    fn track_hover(&self) {
        use objc2::AnyThread;
        use objc2_app_kit::{NSTrackingArea, NSTrackingAreaOptions};
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        if self.trackingAreas().iter().any(|area| area.options().contains(options)) {
            return;
        }
        // SAFETY: The column owns the area and outlives it; AppKit holds the owner weakly.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                options,
                Some(self),
                None,
            )
        };
        self.addTrackingArea(&area);
    }

    /// Draws how `line` differs from the last commit at the right edge of `row`, if `drawn`, and gives its
    /// description as the tooltip of the strip.
    fn change_mark(editor: &EditorTextView, line: usize, row: NSRect, drawn: bool, tips: &mut Vec<Tip>) {
        let Some(mark) = super::changes::mark_at(editor, line) else {
            return;
        };
        if drawn {
            super::changes::draw(mark, row, row.size.width);
        }
        let area = NSRect::new(
            NSPoint::new(row.size.width - CHANGE_STRIP, row.origin.y),
            NSSize::new(CHANGE_STRIP, row.size.height),
        );
        tips.push((area, mark.description().to_string()));
    }

    /// Offers `tips`, unless they are those offered already.
    fn offer_tips(&self, tips: Vec<Tip>) {
        if *self.ivars().offered.borrow() == tips {
            return;
        }
        self.removeAllToolTips();
        let mut owners = self.ivars().tips.borrow_mut();
        owners.clear();
        for (area, text) in &tips {
            let tip = ns(text);
            // SAFETY: The owner is a string, whose description is the tooltip; `tips` keeps it alive while it is shown.
            unsafe { self.addToolTipRect_owner_userData(*area, &tip, std::ptr::null_mut()) };
            owners.push(tip);
        }
        *self.ivars().offered.borrow_mut() = tips;
    }

    /// The attributes and digit widths of the numbers in `font_size`, measured once per size.
    fn looks(&self, font_size: f64) -> std::rc::Rc<Looks> {
        let mut looks = self.ivars().looks.borrow_mut();
        match looks.as_ref() {
            Some(known) if known.font_size == font_size.to_bits() => known.clone(),
            _ => looks.insert(std::rc::Rc::new(Looks::new(font_size))).clone(),
        }
    }

    /// Opens the committed lines of the change mark under a click near the column's right edge.
    fn click_change(&self, event: &NSEvent) -> bool {
        let Some(editor) = self.ivars().editor.load() else {
            return false;
        };
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        let width = self.bounds().size.width;
        if point.x < width - CHANGE_STRIP {
            return false;
        }
        let in_editor = editor.convertPoint_fromView(event.locationInWindow(), None);
        let Some(line) = super::highlight::line_at(&editor, NSPoint::new(0.0, in_editor.y)) else {
            return false;
        };
        let at = NSRect::new(
            NSPoint::new(width - CHANGE_STRIP, point.y - 1.0),
            NSSize::new(CHANGE_STRIP, 2.0),
        );
        super::changes::clicked(&editor, line, self, at)
    }

    /// The column for `editor`, as tall as its text; `viewport` scrolls it with the text.
    fn new(mtm: MainThreadMarker, editor: &EditorTextView) -> Retained<LineNumbers> {
        let this = Self::alloc(mtm).set_ivars(GutterIvars {
            editor: Weak::from(editor),
            tips: std::cell::RefCell::new(vec![]),
            offered: std::cell::RefCell::new(vec![]),
            looks: std::cell::RefCell::new(None),
            hovered: std::cell::Cell::new(false),
            width: std::cell::RefCell::new(None),
            watches: std::cell::RefCell::new(Vec::new()),
        });
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        // Since macOS 14 a view may draw past its bounds, which let a number spill onto the status bar below.
        this.setClipsToBounds(true);
        editor.setPostsFrameChangedNotifications(true);
        let gutter = Weak::from(&*this);
        let name = constant!(NSViewFrameDidChangeNotification);
        let watch = crate::util::observe(mtm, name, Some(editor as &NSView), move |_| {
            if let Some(gutter) = gutter.load() {
                gutter.editor_resized();
            }
        });
        this.ivars().watches.borrow_mut().push(watch);
        this
    }

    /// Draws the rows in `rect`, in the coordinates of the editor's text, and offers the tooltips of those on screen.
    fn draw_numbers(&self, rect: NSRect) {
        NSColor::textBackgroundColor().setFill();
        objc2_app_kit::NSRectFill(rect);
        let Some(editor) = self.ivars().editor.load() else {
            return;
        };
        let shown = rows(&editor);
        let dirty = rows_between(&editor, rect.origin.y, rect.origin.y + rect.size.height);
        let marks = Marks {
            looks: self.looks(editor.ivars().font_size.get()),
            folds: super::fold::gutter_within(&editor, lines_of(&[&shown, &dirty])),
        };
        let mut tips = Vec::new();
        for row in &shown {
            self.draw_row(&editor, &marks, *row, self.band(*row), None, &mut tips);
        }
        let mut hidden_tips = Vec::new();
        for row in &dirty {
            self.draw_row(&editor, &marks, *row, self.band(*row), Some(true), &mut hidden_tips);
        }
        self.offer_tips(tips);
    }

    /// Where `row` lies across the column.
    fn band(&self, row: Row) -> NSRect {
        NSRect::new(
            NSPoint::new(0.0, row.y),
            NSSize::new(self.bounds().size.width, row.height),
        )
    }

    /// Draws `row` at `band` of the column if `number` says whether with its number, and collects its tooltips.
    fn draw_row(
        &self,
        editor: &EditorTextView,
        marks: &Marks,
        row: Row,
        band: NSRect,
        number: Option<bool>,
        tips: &mut Vec<Tip>,
    ) {
        let line = row.line - 1;
        Self::change_mark(editor, line, band, number.is_some(), tips);
        let Some(number) = number else {
            return;
        };
        let fold_strip = NSRect::new(
            NSPoint::new(self.fold_left(), band.origin.y),
            NSSize::new(FOLD_WIDTH, row.height),
        );
        if number {
            let at = NSPoint::new(
                self.fold_left() - NUMBERS_GAP - marks.looks.width(row.line),
                band.origin.y + 1.0,
            );
            // SAFETY: The attribute dictionary holds only a font and a colour under AppKit's keys.
            unsafe { ns(&row.line.to_string()).drawAtPoint_withAttributes(at, Some(&marks.looks.normal)) };
        }
        if let Some(folded) = marks.folds.get(line) {
            self.fold_mark(fold_strip, folded);
        }
    }
}

/// What the column shows beside the lines on screen, gathered once per drawing.
struct Marks {
    looks: std::rc::Rc<Looks>,
    folds: super::fold::Gutter,
}

/// A line's place beside the text: its number from 1, and its top and height in the editor's coordinates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Row {
    pub(crate) line: usize,
    pub(crate) y: f64,
    pub(crate) height: f64,
}

/// The rows of the lines on screen in `editor`, top to bottom.
pub(crate) fn rows(editor: &EditorTextView) -> Vec<Row> {
    let visible = editor.visibleRect();
    let (top, bottom) = (visible.origin.y, visible.origin.y + visible.size.height);
    rows_of(editor, &super::viewport::seen(editor), top, bottom)
}

/// The rows that reach into the heights `top..=bottom` of `editor`, which may lie off screen where the column draws
/// ahead; heights more than a few screens away are left out.
fn rows_between(editor: &EditorTextView, top: f64, bottom: f64) -> Vec<Row> {
    let visible = editor.visibleRect();
    // AppKit draws a scrolled document only a little beyond the screen; laying out more of a long text would stall.
    let reach = visible.size.height.max(1.0) * 3.0;
    let top = top.max(visible.origin.y - reach);
    let bottom = bottom.min(visible.origin.y + visible.size.height + reach);
    if top > bottom {
        return Vec::new();
    }
    rows_of(editor, &super::viewport::seen_between(editor, top, bottom), top, bottom)
}

/// The lines, from 0, that `rows` hold.
fn lines_of(rows: &[&[Row]]) -> std::ops::Range<usize> {
    let first = rows
        .iter()
        .filter_map(|rows| rows.first())
        .map(|row| row.line - 1)
        .min();
    let last = rows.iter().filter_map(|rows| rows.last()).map(|row| row.line).max();
    first.unwrap_or(0)..last.unwrap_or(0)
}

/// The rows of `seen`'s fragments within `top..=bottom`, top to bottom; TextKit lays out no paragraph for an empty
/// text, whose only line takes its height from the font.
fn rows_of(editor: &EditorTextView, seen: &super::viewport::Seen, top: f64, bottom: f64) -> Vec<Row> {
    let last_line = editor.document().text().line_count();
    let mut rows = Vec::new();
    for (place, fragment) in seen.within(top, bottom) {
        let line = place.line + 1;
        let mut row = Row {
            line,
            y: place.rect.origin.y,
            height: place.rect.size.height,
        };
        let after = (place.ends_text && line < last_line).then(|| row_after(fragment, &mut row, last_line));
        rows.extend(std::iter::once(row).chain(after));
    }
    if rows.is_empty() && editor.document().text().len_utf16() == 0 {
        let height = editor
            .font()
            .map_or(0.0, |font| (font.ascender() - font.descender() + font.leading()).ceil());
        rows.push(Row {
            line: 1,
            y: editor.textContainerOrigin().y,
            height,
        });
    }
    rows
}

/// The row of the empty line after a final line break, which TextKit lays out as the last line of `fragment`, the
/// text's last paragraph, not as a paragraph of its own; `row`, the paragraph's, is cut to end where it begins.
fn row_after(fragment: &NSTextLayoutFragment, row: &mut Row, line: usize) -> Row {
    let lines = super::insets::line_fragments(fragment);
    let last = lines.as_ref().and_then(|lines| lines.lastObject());
    match last {
        Some(last) if last.characterRange().length == 0 && lines.as_ref().is_some_and(|lines| lines.count() > 1) => {
            let bounds = last.typographicBounds();
            row.height = bounds.origin.y;
            Row {
                line,
                y: row.y + bounds.origin.y,
                height: bounds.size.height,
            }
        }
        _ => {
            let height = last.map_or(row.height, |last| last.typographicBounds().size.height);
            Row {
                line,
                y: row.y + row.height,
                height,
            }
        }
    }
}

/// The column for `editor` in a scroll view of its own, which only `viewport` scrolls; a wheel over it scrolls the
/// text.
pub(crate) fn column(
    mtm: MainThreadMarker,
    editor: &EditorTextView,
) -> (Retained<NSScrollView>, Retained<LineNumbers>) {
    let numbers = LineNumbers::new(mtm, editor);
    let scroll = NSScrollView::new(mtm);
    scroll.setHasVerticalScroller(false);
    scroll.setHasHorizontalScroller(false);
    scroll.setVerticalScrollElasticity(NSScrollElasticity::None);
    scroll.setHorizontalScrollElasticity(NSScrollElasticity::None);
    scroll.setAutomaticallyAdjustsContentInsets(false);
    scroll.setDrawsBackground(true);
    scroll.setBackgroundColor(&NSColor::textBackgroundColor());
    scroll.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    scroll.setDocumentView(Some(&numbers));
    (scroll, numbers)
}

#[cfg(test)]
mod tests {
    use super::number_width;

    #[test]
    fn a_number_is_as_wide_as_its_digits() {
        let digits = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert!((number_width(0, &digits) - 1.0).abs() < f64::EPSILON);
        assert!((number_width(109, &digits) - 13.0).abs() < f64::EPSILON);
        assert!((number_width(7, &digits) - 8.0).abs() < f64::EPSILON);
    }
}
