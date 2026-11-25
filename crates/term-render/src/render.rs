//! Drawing a terminal screen: the monospaced faces and the cell they fill, then line by line the backgrounds, the
//! selection, the text with its lines, and on top the cursor and text an input method is composing, onto a canvas that
//! is either Core Graphics or the GPU's quads (`gpu`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightRegular};
use objc2_core_foundation::{
    CFAttributedString, CFIndex, CFRange, CFRetained, CFString, CGAffineTransform, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{CGContext, CGGlyph};
use objc2_core_text::{
    CTFont, CTFontOrientation, CTFontSymbolicTraits, CTLine, kCTForegroundColorFromContextAttributeName,
};
use objc2_foundation::{NSMutableAttributedString, NSNumber, NSRange, NSString};
use term_session::{Cell, CellFlags, CursorShape, Highlight, Point, Rgb, Snapshot};

use crate::boxdraw::{self, Pen};
use crate::palette::Theme;
use crate::shape::{self, Shaped, Typography};
use crate::{constant, ns};

/// The space between the cells and the left and right edges of the view.
pub const INSET_X: f64 = 8.0;
/// The space between the cells and the top and bottom edges of the view.
pub const INSET_Y: f64 = 6.0;

/// The four faces come first in the font list, fallback fonts after them.
const FACES: usize = 4;

/// The rectangle at `x`, `y` of `width` and `height`.
pub fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
}

/// The part of `a` inside `b`, empty where they do not meet.
pub fn intersect(a: CGRect, b: CGRect) -> CGRect {
    let left = a.origin.x.max(b.origin.x);
    let top = a.origin.y.max(b.origin.y);
    let right = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let bottom = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    rect(left, top, (right - left).max(0.0), (bottom - top).max(0.0))
}

/// `value` as a count, cut to `0..=max`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the target's range first"
)]
pub fn clamp_to(value: f64, max: usize) -> usize {
    if value.is_nan() {
        return 0;
    }
    value.clamp(0.0, max as f64) as usize
}

/// The size of a cell and where in it the text sits, in points rounded to the screen's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// The width of a cell.
    pub cell_width: f64,
    /// The height of a cell.
    pub cell_height: f64,
    /// The baseline below the top of a cell.
    pub baseline: f64,
    /// The top of an underline below the baseline.
    underline: f64,
    /// The width of underlines, strike lines and light frame lines.
    pub stroke: f64,
    /// The middle of a strike line above the baseline.
    strike: f64,
    /// Device pixels per point.
    pub scale: f64,
}

impl Metrics {
    fn snap(&self, value: f64) -> f64 {
        (value * self.scale).round() / self.scale
    }

    /// The cells from `point` on, `cells` wide.
    pub fn cell_rect(&self, point: Point, cells: usize) -> CGRect {
        rect(
            INSET_X + point.column as f64 * self.cell_width,
            INSET_Y + point.line as f64 * self.cell_height,
            self.cell_width * cells as f64,
            self.cell_height,
        )
    }

    /// The band of `line` across the whole view.
    pub fn line_rect(&self, line: usize, width: f64) -> CGRect {
        rect(0.0, INSET_Y + line as f64 * self.cell_height, width, self.cell_height)
    }

    /// The column and line at `x`, `y`, as fractions; outside the cells they run below 0 or past the last.
    pub fn position(&self, x: f64, y: f64) -> (f64, f64) {
        ((x - INSET_X) / self.cell_width, (y - INSET_Y) / self.cell_height)
    }

    /// The columns and lines that fit into `width` × `height`.
    pub fn fit(&self, width: f64, height: f64) -> (u16, u16) {
        let columns = clamp_to(
            ((width - 2.0 * INSET_X) / self.cell_width).floor(),
            usize::from(u16::MAX),
        );
        let lines = clamp_to(
            ((height - 2.0 * INSET_Y) / self.cell_height).floor(),
            usize::from(u16::MAX),
        );
        (
            u16::try_from(columns).unwrap_or(u16::MAX).max(2),
            u16::try_from(lines).unwrap_or(u16::MAX).max(1),
        )
    }

    /// A length in points as whole device pixels, as the PTY reports cell sizes.
    pub fn pixels(&self, points: f64) -> u16 {
        u16::try_from(clamp_to((points * self.scale).round(), usize::from(u16::MAX))).unwrap_or(u16::MAX)
    }
}

/// A run as shaping left it, none where it needs another font.
type ShapedRun = Option<Rc<[Shaped]>>;

/// One of the four faces of the terminal font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Upright and regular.
    Regular,
    /// Bold.
    Bold,
    /// Italic.
    Italic,
    /// Bold and italic.
    BoldItalic,
}

impl Face {
    pub(crate) fn of(flags: CellFlags) -> Face {
        match (flags.contains(CellFlags::BOLD), flags.contains(CellFlags::ITALIC)) {
            (false, false) => Face::Regular,
            (true, false) => Face::Bold,
            (false, true) => Face::Italic,
            (true, true) => Face::BoldItalic,
        }
    }

    /// The position of the face in `Fonts`.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// A glyph in one of the fonts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// The index of its font in `Fonts`.
    pub font: usize,
    /// The glyph in its font.
    pub glyph: CGGlyph,
    /// How far it moves the pen.
    pub advance: f64,
}

/// The terminal's faces at one size, the fonts that fill in characters they lack, and the glyphs found so far.
pub struct Fonts {
    /// Tells these fonts from any made before, for caches of what was drawn with them.
    pub id: u64,
    fonts: RefCell<Vec<CFRetained<CTFont>>>,
    fallback_names: RefCell<Vec<String>>,
    glyphs: RefCell<HashMap<(char, Face), Option<Glyph>>>,
    typography: Typography,
    shaped: RefCell<HashMap<(Face, String), ShapedRun>>,
    /// The size of a cell and where the text sits in it.
    pub metrics: Metrics,
}

impl Fonts {
    /// The system's monospaced faces at `size` points for a screen with `scale` pixels per point.
    pub fn new(size: f64, scale: f64) -> Fonts {
        Fonts::with_family("", size, scale)
    }

    /// The faces of `family` at `size` points; the system's monospaced ones when the family is not installed or
    /// is SF Mono, which the system hands out only by its weight.
    pub fn with_family(family: &str, size: f64, scale: f64) -> Fonts {
        Fonts::styled(family, size, scale, &Typography::default())
    }

    /// The faces of `family` at `size` points, set with the features and axes of `typography` (F13).
    pub fn styled(family: &str, size: f64, scale: f64, typography: &Typography) -> Fonts {
        let named = (!family.is_empty() && family != "SF Mono")
            .then(|| NSFont::fontWithName_size(&crate::ns(family), size))
            .flatten()
            .map(bridged);
        let (regular, bold) = match named {
            Some(regular) => {
                let bold = bold_of(&regular);
                (regular, bold)
            }
            None => (
                system_mono(size, constant!(NSFontWeightRegular)),
                system_mono(size, constant!(NSFontWeightBold)),
            ),
        };
        let italic = shape::styled(italic_of(&regular), typography, false);
        let bold_italic = shape::styled(italic_of(&bold), typography, true);
        let regular = shape::styled(regular, typography, false);
        let bold = shape::styled(bold, typography, true);
        let metrics = measure(&regular, scale.max(1.0));
        static MADE: AtomicU64 = AtomicU64::new(0);
        Fonts {
            id: MADE.fetch_add(1, Ordering::Relaxed),
            fonts: RefCell::new(vec![regular, bold, italic, bold_italic]),
            fallback_names: RefCell::new(vec![]),
            glyphs: RefCell::new(HashMap::new()),
            typography: typography.clone(),
            shaped: RefCell::new(HashMap::new()),
            metrics,
        }
    }

    /// Text is shaped before it is drawn.
    pub fn shapes(&self) -> bool {
        self.typography.shapes()
    }

    /// `text` shaped in `face`, one character per cell; none where shaping needs another font.
    pub(crate) fn shaped(&self, text: &str, face: Face) -> Option<Rc<[Shaped]>> {
        if let Some(found) = self.shaped.borrow().get(&(face, text.to_string())) {
            return found.clone();
        }
        let advance = self.glyph('M', face).map_or(self.metrics.cell_width, |m| m.advance);
        let found: Option<Rc<[Shaped]>> = shape::shape(&self.font(face.index()), text, advance).map(Into::into);
        let mut shaped = self.shaped.borrow_mut();
        if shaped.len() >= 4096 {
            shaped.clear();
        }
        shaped.insert((face, text.to_string()), found.clone());
        found
    }

    /// The font at `index`.
    pub fn font(&self, index: usize) -> CFRetained<CTFont> {
        self.fonts.borrow()[index].clone()
    }

    /// The glyph for `c` in `face`, from a fallback font where the face lacks it.
    pub fn glyph(&self, c: char, face: Face) -> Option<Glyph> {
        if let Some(found) = self.glyphs.borrow().get(&(c, face)) {
            return *found;
        }
        let found = self.find(c, face);
        self.glyphs.borrow_mut().insert((c, face), found);
        found
    }

    fn find(&self, c: char, face: Face) -> Option<Glyph> {
        let mut units = [0u16; 2];
        let units = &*c.encode_utf16(&mut units);
        let primary = self.font(face.index());
        if let Some(glyph) = glyph_in(&primary, units) {
            return Some(Glyph {
                font: face.index(),
                glyph,
                advance: advance_of(&primary, glyph),
            });
        }
        let text = CFString::from_str(c.encode_utf8(&mut [0; 4]));
        let range = CFRange {
            location: 0,
            length: CFIndex::try_from(units.len()).unwrap_or(1),
        };
        // SAFETY: The range covers the string, which holds the one character.
        let fallback = unsafe { primary.for_string(&text, range) };
        let glyph = glyph_in(&fallback, units)?;
        Some(Glyph {
            font: self.add_fallback(fallback.clone()),
            glyph,
            advance: advance_of(&fallback, glyph),
        })
    }

    fn add_fallback(&self, font: CFRetained<CTFont>) -> usize {
        // SAFETY: A font always has a PostScript name.
        let name = unsafe { font.post_script_name() }.to_string();
        let mut names = self.fallback_names.borrow_mut();
        if let Some(index) = names.iter().position(|known| *known == name) {
            return FACES + index;
        }
        names.push(name);
        self.fonts.borrow_mut().push(font);
        FACES + names.len() - 1
    }
}

fn system_mono(size: f64, weight: f64) -> CFRetained<CTFont> {
    bridged(NSFont::monospacedSystemFontOfSize_weight(size, weight))
}

/// The bold of `font`, or the font itself where its family has none.
fn bold_of(font: &CFRetained<CTFont>) -> CFRetained<CTFont> {
    let bold = CTFontSymbolicTraits::BoldTrait;
    // SAFETY: A null matrix keeps the font's own.
    let found = unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), bold, bold) };
    found.unwrap_or_else(|| font.clone())
}

fn bridged(font: Retained<NSFont>) -> CFRetained<CTFont> {
    let raw = Retained::into_raw(font).cast::<CTFont>();
    // SAFETY: NSFont and CTFont are toll-free bridged, so the NSFont's reference is one to the same CTFont, which the
    // pointer from `into_raw` is never null for.
    unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw)) }
}

/// The italic of `font`, or the font slanted where its family has none.
fn italic_of(font: &CTFont) -> CFRetained<CTFont> {
    let italic = CTFontSymbolicTraits::ItalicTrait;
    // SAFETY: A null matrix keeps the font's own.
    let found = unsafe { font.copy_with_symbolic_traits(0.0, std::ptr::null(), italic, italic) };
    found.unwrap_or_else(|| {
        let slant = CGAffineTransform {
            a: 1.0,
            b: 0.0,
            c: 0.21,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        };
        // SAFETY: The matrix lives on the stack for the length of the call.
        unsafe { font.copy_with_attributes(0.0, &raw const slant, None) }
    })
}

fn glyph_in(font: &CTFont, units: &[u16]) -> Option<CGGlyph> {
    let mut glyphs: [CGGlyph; 2] = [0; 2];
    let count = CFIndex::try_from(units.len()).ok()?;
    // SAFETY: Both buffers hold at least `units.len()` entries, which is one or two.
    let mapped =
        unsafe { font.glyphs_for_characters(NonNull::from(units).cast(), NonNull::from(&mut glyphs).cast(), count) };
    (mapped && glyphs[0] != 0).then_some(glyphs[0])
}

fn advance_of(font: &CTFont, glyph: CGGlyph) -> f64 {
    let mut glyph = glyph;
    // SAFETY: One glyph, and a null buffer for the advances asks only for the total.
    unsafe {
        font.advances_for_glyphs(
            CTFontOrientation::Horizontal,
            NonNull::from(&mut glyph),
            std::ptr::null_mut(),
            1,
        )
    }
}

fn measure(font: &CTFont, scale: f64) -> Metrics {
    // SAFETY: Metric getters of a valid font.
    let (ascent, descent, leading, underline, thickness, x_height) = unsafe {
        (
            font.ascent(),
            font.descent(),
            font.leading(),
            font.underline_position(),
            font.underline_thickness(),
            font.x_height(),
        )
    };
    let snap = |value: f64| (value * scale).round() / scale;
    let advance = glyph_in(font, &[u16::from(b'M')]).map_or(ascent * 0.6, |glyph| advance_of(font, glyph));
    let cell_width = snap(advance).max(1.0 / scale);
    let cell_height = ((ascent + descent + leading) * scale).ceil() / scale;
    let baseline = snap((cell_height - ascent - descent) / 2.0 + ascent);
    let stroke = (thickness * scale).round().max(1.0) / scale;
    let underline = snap(-underline - stroke / 2.0).clamp(0.0, (cell_height - baseline - stroke).max(0.0));
    Metrics {
        cell_width,
        cell_height,
        baseline,
        underline,
        stroke,
        strike: x_height / 2.0,
        scale,
    }
}

/// Fills `area` with `color`.
pub fn fill(ctx: &CGContext, color: Rgb, area: CGRect) {
    set_color(ctx, color, 1.0);
    CGContext::fill_rect(Some(ctx), area);
}

/// Makes `color` at `alpha` what the context fills and strokes with.
pub fn set_color(ctx: &CGContext, color: Rgb, alpha: f64) {
    let channel = |v: u8| f64::from(v) / 255.0;
    let (r, g, b) = (channel(color.r), channel(color.g), channel(color.b));
    CGContext::set_rgb_fill_color(Some(ctx), r, g, b, alpha);
    CGContext::set_rgb_stroke_color(Some(ctx), r, g, b, alpha);
}

/// What a frame shows besides the screen itself.
pub struct Overlay<'a> {
    /// The view has the keyboard focus and its window is the key window.
    pub focused: bool,
    /// The cursor is in the visible phase of its blinking.
    pub cursor_on: bool,
    /// Something of the view stands in for the cursor, such as copy mode's, so the program's is not drawn.
    pub hide_cursor: bool,
    /// Text an input method is composing, shown at the cursor.
    pub marked: &'a str,
    /// A line drawn after the program ended, and where.
    pub final_line: Option<(usize, &'a str)>,
    /// The bell's flash.
    pub flash: bool,
}

/// What a frame is drawn onto, in points from the view's top left: a Core Graphics context, or the GPU's quads.
pub trait Canvas {
    /// Fills `area` with `color` at `alpha`.
    fn fill(&mut self, color: Rgb, alpha: f64, area: CGRect);
    /// `glyph` with its origin at `x` on `baseline`.
    fn glyph(&mut self, glyph: Glyph, x: f64, baseline: f64, color: Rgb);
    /// Draws `c` into `pen`'s cells if it is a line or block character drawn by hand; whether it is one.
    fn boxdraw(&mut self, c: char, pen: &Pen, color: Rgb) -> bool;
    /// How wide `text` is in `face`.
    fn measure(&mut self, text: &str, face: Face) -> f64;
    /// `text` shaped by Core Text in `face`, starting at `x` on `baseline`.
    fn line(&mut self, text: &str, face: Face, x: f64, baseline: f64, color: Rgb);
    /// An underline of `kind` across `area` whose top is at `y`.
    fn underline(&mut self, kind: Stroke, area: CGRect, y: f64, color: Rgb);
}

/// Glyphs in one font, colour and baseline, drawn with one call.
struct Batch {
    font: usize,
    color: Rgb,
    baseline: f64,
    glyphs: Vec<CGGlyph>,
    positions: Vec<CGPoint>,
}

/// Draws straight into a Core Graphics context, as `drawRect:` and pictures of the view do.
pub struct CgCanvas<'a> {
    ctx: &'a CGContext,
    fonts: &'a Fonts,
    batch: Option<Batch>,
}

impl CgCanvas<'_> {
    fn flush(&mut self) {
        let Some(batch) = self.batch.take() else { return };
        let font = self.fonts.font(batch.font);
        set_color(self.ctx, batch.color, 1.0);
        show_glyphs(self.ctx, &font, &batch.glyphs, &batch.positions, batch.baseline);
    }
}

impl Drop for CgCanvas<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

impl Canvas for CgCanvas<'_> {
    fn fill(&mut self, color: Rgb, alpha: f64, area: CGRect) {
        self.flush();
        set_color(self.ctx, color, alpha);
        CGContext::fill_rect(Some(self.ctx), area);
    }

    fn glyph(&mut self, glyph: Glyph, x: f64, baseline: f64, color: Rgb) {
        let position = CGPoint::new(x, 0.0);
        if let Some(open) = &mut self.batch
            && open.font == glyph.font
            && open.color == color
            && (open.baseline - baseline).abs() < f64::EPSILON
        {
            open.glyphs.push(glyph.glyph);
            open.positions.push(position);
            return;
        }
        self.flush();
        self.batch = Some(Batch {
            font: glyph.font,
            color,
            baseline,
            glyphs: vec![glyph.glyph],
            positions: vec![position],
        });
    }

    fn boxdraw(&mut self, c: char, pen: &Pen, color: Rgb) -> bool {
        if !boxdraw::covers(c) {
            return false;
        }
        self.flush();
        set_color(self.ctx, color, 1.0);
        boxdraw::draw(self.ctx, c, pen)
    }

    fn measure(&mut self, text: &str, face: Face) -> f64 {
        line_width(&line_of(self.fonts, text, face))
    }

    fn line(&mut self, text: &str, face: Face, x: f64, baseline: f64, color: Rgb) {
        self.flush();
        draw_line(self.ctx, &line_of(self.fonts, text, face), x, baseline, color);
    }

    fn underline(&mut self, kind: Stroke, area: CGRect, y: f64, color: Rgb) {
        self.flush();
        set_color(self.ctx, color, 1.0);
        draw_underline(self.ctx, &self.fonts.metrics, kind, area, y, 0.0);
    }
}

/// Draws the lines of `screen` that meet `dirty` with Core Graphics, and over them what `overlay` adds.
pub fn draw(ctx: &CGContext, fonts: &Fonts, theme: &Theme, screen: &Snapshot, overlay: &Overlay, dirty: CGRect) {
    let mut canvas = CgCanvas {
        ctx,
        fonts,
        batch: None,
    };
    paint(&mut canvas, fonts, theme, screen, overlay, dirty);
}

/// Paints the lines of `screen` that meet `dirty` onto `canvas`, and over them what `overlay` adds.
pub fn paint(
    canvas: &mut impl Canvas,
    fonts: &Fonts,
    theme: &Theme,
    screen: &Snapshot,
    overlay: &Overlay,
    dirty: CGRect,
) {
    let m = fonts.metrics;
    canvas.fill(screen.background, 1.0, dirty);
    let width = dirty.origin.x + dirty.size.width;
    let (_, top) = m.position(0.0, dirty.origin.y);
    let (_, bottom) = m.position(0.0, dirty.origin.y + dirty.size.height);
    let first = clamp_to(top.floor(), screen.lines);
    let last = clamp_to(bottom.ceil(), screen.lines);
    for line in first..last {
        paint_line(canvas, fonts, theme, screen, line, width);
    }
    paint_overlay(canvas, fonts, theme, screen, overlay, dirty);
}

/// Line `line` of `screen`: backgrounds, the selection over them, the text and its decorations; on the GPU the quads
/// of a line that did not change are kept from the frame before.
pub fn paint_line(canvas: &mut impl Canvas, fonts: &Fonts, theme: &Theme, screen: &Snapshot, line: usize, width: f64) {
    let m = fonts.metrics;
    if let Some(row) = screen.rows.get(line) {
        backgrounds(canvas, &m, screen.background, line, row, width);
        if let Some((from, to)) = selected_columns(screen, line) {
            let area = m.cell_rect(Point { line, column: from }, to + 1 - from);
            canvas.fill(theme.selection, 1.0, area);
        }
        text(canvas, fonts, theme, line, row, 0..row.len(), None);
        decorations(canvas, &m, line, row);
    }
}

/// The selected columns of line `line`, first to last.
pub fn selected_columns(screen: &Snapshot, line: usize) -> Option<(usize, usize)> {
    let row = screen.rows.get(line)?;
    screen.selection.and_then(|s| selected(&s, line, row.len()))
}

/// What lies over the lines: the line after the program ended, the cursor, composed text and the bell's flash.
pub fn paint_overlay(
    canvas: &mut impl Canvas,
    fonts: &Fonts,
    theme: &Theme,
    screen: &Snapshot,
    overlay: &Overlay,
    dirty: CGRect,
) {
    let m = fonts.metrics;
    if let Some((line, words)) = overlay.final_line {
        let x = m.cell_rect(Point { line, column: 0 }, 1).origin.x;
        let baseline = INSET_Y + line as f64 * m.cell_height + m.baseline;
        canvas.line(words, Face::Regular, x, baseline, theme.palette.foreground.dim());
    } else if overlay.marked.is_empty() {
        cursor(canvas, fonts, theme, screen, overlay);
    }
    marked(canvas, fonts, theme, screen, overlay.marked);
    if overlay.flash {
        canvas.fill(theme.palette.foreground, 0.12, dirty);
    }
}

/// The selected columns of `line`, first and last.
fn selected(selection: &Highlight, line: usize, columns: usize) -> Option<(usize, usize)> {
    if line < selection.start.line || line > selection.end.line || columns == 0 {
        return None;
    }
    if selection.block {
        let (a, b) = (selection.start.column, selection.end.column);
        return Some((a.min(b), a.max(b).min(columns - 1)));
    }
    let from = if line == selection.start.line {
        selection.start.column
    } else {
        0
    };
    let to = if line == selection.end.line {
        selection.end.column.min(columns - 1)
    } else {
        columns - 1
    };
    (from <= to).then_some((from, to))
}

/// Cell backgrounds other than the screen's, the outer ones reaching to the edges of the view.
fn backgrounds(canvas: &mut impl Canvas, m: &Metrics, default: Rgb, line: usize, row: &[Cell], width: f64) {
    let mut start = 0;
    while start < row.len() {
        let bg = row[start].bg;
        let end = (start..row.len()).find(|&i| row[i].bg != bg).unwrap_or(row.len());
        if bg != default {
            let mut area = m.cell_rect(Point { line, column: start }, end - start);
            if start == 0 {
                area.size.width += area.origin.x;
                area.origin.x = 0.0;
            }
            if end == row.len() {
                area.size.width = (width - area.origin.x).max(area.size.width);
            }
            canvas.fill(bg, 1.0, area);
        }
        start = end;
    }
}

/// The text of `columns` of `row`, in `color` if given, else each cell's own kept readable on its background.
fn text(
    canvas: &mut impl Canvas,
    fonts: &Fonts,
    theme: &Theme,
    line: usize,
    row: &[Cell],
    columns: std::ops::Range<usize>,
    color: Option<Rgb>,
) {
    let m = fonts.metrics;
    let baseline = INSET_Y + line as f64 * m.cell_height + m.baseline;
    let mut shaped_to = columns.start;
    for column in columns.clone() {
        if column < shaped_to {
            continue;
        }
        if fonts.shapes() {
            let run = shape::Run { line, baseline, color };
            if let Some(end) = run.draw(canvas, fonts, theme, row, column..columns.end) {
                shaped_to = end;
                continue;
            }
        }
        let Some(cell) = row.get(column) else { break };
        if cell.flags.contains(CellFlags::WIDE_SPACER)
            || cell.flags.contains(CellFlags::HIDDEN)
            || (cell.zero_width.is_empty() && (cell.ch == ' ' || cell.ch == '\0'))
        {
            continue;
        }
        let fg = color.unwrap_or_else(|| theme.readable(cell.fg, cell.bg));
        let cells = if cell.flags.contains(CellFlags::WIDE) { 2 } else { 1 };
        let area = m.cell_rect(Point { line, column }, cells);
        let face = Face::of(cell.flags);
        if cell.zero_width.is_empty() {
            let pen = Pen {
                rect: area,
                scale: m.scale,
                light: m.stroke,
            };
            if canvas.boxdraw(cell.ch, &pen, fg) {
                continue;
            }
            if let Some(glyph) = fonts.glyph(cell.ch, face) {
                let offset = if glyph.font >= FACES && glyph.advance < area.size.width {
                    (area.size.width - glyph.advance) / 2.0
                } else {
                    0.0
                };
                canvas.glyph(glyph, area.origin.x + offset, baseline, fg);
                continue;
            }
        }
        let cluster: String = std::iter::once(cell.ch)
            .chain(cell.zero_width.iter().copied())
            .collect();
        let width = canvas.measure(&cluster, face);
        let offset = ((area.size.width - width) / 2.0).max(0.0);
        canvas.line(&cluster, face, area.origin.x + offset, baseline, fg);
    }
}

/// Draws `glyphs` of `font` at `positions` along `baseline`, in a space turned upright for them.
pub fn show_glyphs(ctx: &CGContext, font: &CTFont, glyphs: &[CGGlyph], positions: &[CGPoint], baseline: f64) {
    let count = glyphs.len().min(positions.len());
    let c = Some(ctx);
    CGContext::save_g_state(c);
    CGContext::translate_ctm(c, 0.0, baseline);
    CGContext::scale_ctm(c, 1.0, -1.0);
    upright_text(ctx);
    // SAFETY: Both buffers hold at least `count` entries.
    unsafe {
        font.draw_glyphs(
            NonNull::from(glyphs).cast(),
            NonNull::from(positions).cast(),
            count,
            ctx,
        );
    }
    CGContext::restore_g_state(c);
}

/// Resets the text matrix, which AppKit leaves flipped for flipped views and whose translation is the position where
/// the last text drawn into the context ended; glyph positions are taken relative to it.
fn upright_text(ctx: &CGContext) {
    let identity = CGAffineTransform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };
    CGContext::set_text_matrix(Some(ctx), identity);
}

/// A line of Core Text in `face` whose glyphs take the context's fill colour.
pub fn line_of(fonts: &Fonts, text: &str, face: Face) -> CFRetained<CTLine> {
    let font = fonts.font(face.index());
    let string = NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &ns(text));
    let whole = NSRange::new(0, string.length());
    // SAFETY: CTFont and NSFont are toll-free bridged, as are CFString and NSString.
    let (font, from_context) = unsafe {
        (
            &*std::ptr::from_ref::<CTFont>(&font).cast::<NSFont>(),
            &*std::ptr::from_ref::<CFString>(kCTForegroundColorFromContextAttributeName).cast::<NSString>(),
        )
    };
    let font_key = constant!(NSFontAttributeName);
    // SAFETY: The font key takes a font, Core Text's context colour key a boolean number.
    unsafe {
        string.addAttribute_value_range(font_key, font, whole);
        string.addAttribute_value_range(from_context, &NSNumber::numberWithBool(true), whole);
    }
    // SAFETY: NSAttributedString and CFAttributedString are toll-free bridged.
    let attributed = unsafe { &*Retained::as_ptr(&string).cast::<CFAttributedString>() };
    // SAFETY: The attributed string is valid for the call.
    unsafe { CTLine::with_attributed_string(attributed) }
}

/// How wide `line` is drawn.
pub fn line_width(line: &CTLine) -> f64 {
    // SAFETY: Null pointers skip the metrics not asked for.
    unsafe { line.typographic_bounds(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) }
}

/// Draws `line` in `color` with its baseline at `baseline`, starting at `x`.
pub fn draw_line(ctx: &CGContext, line: &CTLine, x: f64, baseline: f64, color: Rgb) {
    set_color(ctx, color, 1.0);
    let c = Some(ctx);
    CGContext::save_g_state(c);
    CGContext::translate_ctm(c, x, baseline);
    CGContext::scale_ctm(c, 1.0, -1.0);
    upright_text(ctx);
    // SAFETY: The context is the one being drawn into.
    unsafe { line.draw(ctx) };
    CGContext::restore_g_state(c);
}

/// The kinds of underline a cell can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stroke {
    /// One line.
    Single,
    /// Two lines.
    Double,
    /// A wave.
    Curly,
    /// Dots.
    Dotted,
    /// Dashes.
    Dashed,
}

impl Stroke {
    /// The length after which a dotted or dashed line repeats, for a line `width` thick.
    pub fn period(self, width: f64) -> Option<f64> {
        match self {
            Stroke::Dotted => Some(2.0 * width),
            Stroke::Dashed => Some(5.0 * width),
            Stroke::Single | Stroke::Double | Stroke::Curly => None,
        }
    }
}

fn underline_of(flags: CellFlags) -> Option<Stroke> {
    [
        (CellFlags::UNDERCURL, Stroke::Curly),
        (CellFlags::DOUBLE_UNDERLINE, Stroke::Double),
        (CellFlags::DOTTED_UNDERLINE, Stroke::Dotted),
        (CellFlags::DASHED_UNDERLINE, Stroke::Dashed),
        (CellFlags::UNDERLINE, Stroke::Single),
    ]
    .into_iter()
    .find(|(flag, _)| flags.contains(*flag))
    .map(|(_, stroke)| stroke)
}

/// Underlines and strike lines, each run of cells that share one drawn at once.
fn decorations(canvas: &mut impl Canvas, m: &Metrics, line: usize, row: &[Cell]) {
    let top = INSET_Y + line as f64 * m.cell_height;
    let baseline = top + m.baseline;
    let mut column = 0;
    while column < row.len() {
        let cell = &row[column];
        let underline = underline_of(cell.flags).map(|kind| (kind, cell.underline_color.unwrap_or(cell.fg)));
        let strike = cell.flags.contains(CellFlags::STRIKETHROUGH).then_some(cell.fg);
        let same = |other: &Cell| {
            underline_of(other.flags).map(|kind| (kind, other.underline_color.unwrap_or(other.fg))) == underline
                && other.flags.contains(CellFlags::STRIKETHROUGH).then_some(other.fg) == strike
        };
        let end = (column + 1..row.len()).find(|&i| !same(&row[i])).unwrap_or(row.len());
        let area = m.cell_rect(Point { line, column }, end - column);
        if let Some(color) = strike {
            let y = m.snap(baseline - m.strike - m.stroke / 2.0);
            canvas.fill(color, 1.0, rect(area.origin.x, y, area.size.width, m.stroke));
        }
        if let Some((kind, color)) = underline {
            canvas.underline(kind, area, baseline + m.underline, color);
        }
        column = end;
    }
}

/// Strokes an underline of `kind` across `area` with its top at `y` in the context's colour; dotted and dashed lines
/// start `phase` into their pattern.
pub fn draw_underline(ctx: &CGContext, m: &Metrics, kind: Stroke, area: CGRect, y: f64, phase: f64) {
    let (x, width, t) = (area.origin.x, area.size.width, m.stroke);
    let c = Some(ctx);
    match kind {
        Stroke::Single => CGContext::fill_rect(c, rect(x, y, width, t)),
        Stroke::Double => {
            let below = (y + 2.0 * t).min(area.origin.y + area.size.height - t);
            CGContext::fill_rect(c, rect(x, below - 2.0 * t, width, t));
            CGContext::fill_rect(c, rect(x, below, width, t));
        }
        Stroke::Dotted | Stroke::Dashed => {
            let pattern = if kind == Stroke::Dotted {
                [t, t]
            } else {
                [3.0 * t, 2.0 * t]
            };
            CGContext::save_g_state(c);
            CGContext::set_line_width(c, t);
            // SAFETY: The pattern holds the two lengths passed.
            unsafe { CGContext::set_line_dash(c, phase, pattern.as_ptr(), 2) };
            CGContext::begin_path(c);
            CGContext::move_to_point(c, x, y + t / 2.0);
            CGContext::add_line_to_point(c, x + width, y + t / 2.0);
            CGContext::stroke_path(c);
            CGContext::restore_g_state(c);
        }
        Stroke::Curly => {
            let wave = (m.cell_width / 2.0).max(2.0);
            let height = (2.0 * t).max(1.5);
            CGContext::set_line_width(c, t);
            CGContext::begin_path(c);
            CGContext::move_to_point(c, x, y + height / 2.0);
            let mut from = x;
            let mut up = true;
            while from < x + width {
                let to = (from + wave).min(x + width);
                let peak = if up { y - height / 2.0 } else { y + height * 1.5 };
                CGContext::add_quad_curve_to_point(c, (from + to) / 2.0, peak, to, y + height / 2.0);
                from = to;
                up = !up;
            }
            CGContext::stroke_path(c);
        }
    }
}

fn cursor(canvas: &mut impl Canvas, fonts: &Fonts, theme: &Theme, screen: &Snapshot, overlay: &Overlay) {
    let at = screen.cursor;
    if !at.visible || overlay.hide_cursor || (overlay.focused && !overlay.cursor_on) {
        return;
    }
    let m = fonts.metrics;
    let color = theme.palette.cursor.unwrap_or(theme.palette.foreground);
    let area = m.cell_rect(at.point, if at.wide { 2 } else { 1 });
    let shape = if overlay.focused {
        at.shape
    } else {
        CursorShape::HollowBlock
    };
    let bar = m.snap((m.cell_width / 8.0).max(1.0)).max(1.0 / m.scale);
    let (x, y, w, h, s) = (
        area.origin.x,
        area.origin.y,
        area.size.width,
        area.size.height,
        m.stroke,
    );
    match shape {
        CursorShape::Block => {
            canvas.fill(color, 1.0, area);
            if let Some(row) = screen.rows.get(at.point.line) {
                let behind = row.get(at.point.column).map_or(screen.background, |cell| cell.bg);
                let column = at.point.column;
                text(
                    canvas,
                    fonts,
                    theme,
                    at.point.line,
                    row,
                    column..column + 1,
                    Some(behind),
                );
            }
        }
        CursorShape::HollowBlock => {
            for side in [
                rect(x, y, w, s),
                rect(x, y + h - s, w, s),
                rect(x, y + s, s, h - 2.0 * s),
                rect(x + w - s, y + s, s, h - 2.0 * s),
            ] {
                canvas.fill(color, 1.0, side);
            }
        }
        CursorShape::Beam => canvas.fill(color, 1.0, rect(x, y, bar, h)),
        CursorShape::Underline => canvas.fill(color, 1.0, rect(x, y + h - bar, w, bar)),
    }
}

/// Text an input method is composing, on the screen's background at the cursor and underlined.
fn marked(canvas: &mut impl Canvas, fonts: &Fonts, theme: &Theme, screen: &Snapshot, text: &str) {
    if text.is_empty() {
        return;
    }
    let m = fonts.metrics;
    let area = m.cell_rect(screen.cursor.point, 1);
    let width = canvas.measure(text, Face::Regular).max(m.cell_width);
    canvas.fill(
        screen.background,
        1.0,
        rect(area.origin.x, area.origin.y, width, area.size.height),
    );
    let baseline = area.origin.y + m.baseline;
    canvas.line(text, Face::Regular, area.origin.x, baseline, theme.palette.foreground);
    canvas.fill(
        theme.palette.foreground,
        1.0,
        rect(area.origin.x, baseline + m.underline, width, m.stroke),
    );
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
