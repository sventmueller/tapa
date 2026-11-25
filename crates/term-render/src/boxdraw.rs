//! Line and block characters drawn as shapes filling their cell, so frames and bars join without the gaps a font's
//! glyphs leave between lines.

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGContext, CGLineCap};

/// The arms of a line character: up, right, down, left; 1 is light, 2 heavy.
type Arms = [u8; 4];

/// U+2500 to U+254B; `None` for the dashed lines, which `dashes` covers.
const LINES: [Option<Arms>; 76] = {
    const fn a(up: u8, right: u8, down: u8, left: u8) -> Option<Arms> {
        Some([up, right, down, left])
    }
    [
        a(0, 1, 0, 1),
        a(0, 2, 0, 2),
        a(1, 0, 1, 0),
        a(2, 0, 2, 0),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        a(0, 1, 1, 0),
        a(0, 2, 1, 0),
        a(0, 1, 2, 0),
        a(0, 2, 2, 0),
        a(0, 0, 1, 1),
        a(0, 0, 1, 2),
        a(0, 0, 2, 1),
        a(0, 0, 2, 2),
        a(1, 1, 0, 0),
        a(1, 2, 0, 0),
        a(2, 1, 0, 0),
        a(2, 2, 0, 0),
        a(1, 0, 0, 1),
        a(1, 0, 0, 2),
        a(2, 0, 0, 1),
        a(2, 0, 0, 2),
        a(1, 1, 1, 0),
        a(1, 2, 1, 0),
        a(2, 1, 1, 0),
        a(1, 1, 2, 0),
        a(2, 1, 2, 0),
        a(2, 2, 1, 0),
        a(1, 2, 2, 0),
        a(2, 2, 2, 0),
        a(1, 0, 1, 1),
        a(1, 0, 1, 2),
        a(2, 0, 1, 1),
        a(1, 0, 2, 1),
        a(2, 0, 2, 1),
        a(2, 0, 1, 2),
        a(1, 0, 2, 2),
        a(2, 0, 2, 2),
        a(0, 1, 1, 1),
        a(0, 1, 1, 2),
        a(0, 2, 1, 1),
        a(0, 2, 1, 2),
        a(0, 1, 2, 1),
        a(0, 1, 2, 2),
        a(0, 2, 2, 1),
        a(0, 2, 2, 2),
        a(1, 1, 0, 1),
        a(1, 1, 0, 2),
        a(1, 2, 0, 1),
        a(1, 2, 0, 2),
        a(2, 1, 0, 1),
        a(2, 1, 0, 2),
        a(2, 2, 0, 1),
        a(2, 2, 0, 2),
        a(1, 1, 1, 1),
        a(1, 1, 1, 2),
        a(1, 2, 1, 1),
        a(1, 2, 1, 2),
        a(2, 1, 1, 1),
        a(1, 1, 2, 1),
        a(2, 1, 2, 1),
        a(2, 1, 1, 2),
        a(2, 2, 1, 1),
        a(1, 1, 2, 2),
        a(1, 2, 2, 1),
        a(2, 2, 1, 2),
        a(1, 2, 2, 2),
        a(2, 1, 2, 2),
        a(2, 2, 2, 1),
        a(2, 2, 2, 2),
    ]
};

/// U+2574 to U+257F: half lines and lines that change weight in the middle.
const HALVES: [Arms; 12] = [
    [0, 0, 0, 1],
    [1, 0, 0, 0],
    [0, 1, 0, 0],
    [0, 0, 1, 0],
    [0, 0, 0, 2],
    [2, 0, 0, 0],
    [0, 2, 0, 0],
    [0, 0, 2, 0],
    [0, 2, 0, 1],
    [1, 0, 2, 0],
    [0, 1, 0, 2],
    [2, 0, 1, 0],
];

/// U+2550 to U+256C: arms up, right, down, left; 1 is a single line, 2 a double one.
const DOUBLES: [Arms; 29] = [
    [0, 2, 0, 2],
    [2, 0, 2, 0],
    [0, 2, 1, 0],
    [0, 1, 2, 0],
    [0, 2, 2, 0],
    [0, 0, 1, 2],
    [0, 0, 2, 1],
    [0, 0, 2, 2],
    [1, 2, 0, 0],
    [2, 1, 0, 0],
    [2, 2, 0, 0],
    [1, 0, 0, 2],
    [2, 0, 0, 1],
    [2, 0, 0, 2],
    [1, 2, 1, 0],
    [2, 1, 2, 0],
    [2, 2, 2, 0],
    [1, 0, 1, 2],
    [2, 0, 2, 1],
    [2, 0, 2, 2],
    [0, 2, 1, 2],
    [0, 1, 2, 1],
    [0, 2, 2, 2],
    [1, 2, 0, 2],
    [2, 1, 0, 1],
    [2, 2, 0, 2],
    [1, 2, 1, 2],
    [2, 1, 2, 1],
    [2, 2, 2, 2],
];

/// A dashed line: across or down, heavy or light, and how many dashes fill a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dashes {
    across: bool,
    heavy: bool,
    count: u8,
}

fn dashes(code: u32) -> Option<Dashes> {
    let (across, heavy, count) = match code {
        0x2504 => (true, false, 3),
        0x2505 => (true, true, 3),
        0x2506 => (false, false, 3),
        0x2507 => (false, true, 3),
        0x2508 => (true, false, 4),
        0x2509 => (true, true, 4),
        0x250a => (false, false, 4),
        0x250b => (false, true, 4),
        0x254c => (true, false, 2),
        0x254d => (true, true, 2),
        0x254e => (false, false, 2),
        0x254f => (false, true, 2),
        _ => return None,
    };
    Some(Dashes { across, heavy, count })
}

/// How a cell is filled: its rectangle, the device scale and the width of a light line.
#[derive(Clone, Copy)]
pub struct Pen {
    /// The cell drawn into.
    pub rect: CGRect,
    /// Pixels per point.
    pub scale: f64,
    /// The width of a light line in points.
    pub light: f64,
}

impl Pen {
    fn snap(&self, value: f64) -> f64 {
        (value * self.scale).round() / self.scale
    }

    /// A line `width` wide across the cell from `from` to `to`, centred on `y`.
    fn across(&self, ctx: &CGContext, from: f64, to: f64, y: f64, width: f64) {
        let (from, to, top) = (self.snap(from), self.snap(to), self.snap(y - width / 2.0));
        if to > from {
            CGContext::fill_rect(Some(ctx), rect(from, top, to - from, width));
        }
    }

    /// A line `width` wide down the cell from `from` to `to`, centred on `x`.
    fn down(&self, ctx: &CGContext, from: f64, to: f64, x: f64, width: f64) {
        let (from, to, left) = (self.snap(from), self.snap(to), self.snap(x - width / 2.0));
        if to > from {
            CGContext::fill_rect(Some(ctx), rect(left, from, width, to - from));
        }
    }

    /// Fills the part of the cell from `left` to `right` and `top` to `bottom`, as fractions of its size.
    fn fill(&self, ctx: &CGContext, left: f64, top: f64, right: f64, bottom: f64) {
        let r = self.rect;
        let x0 = self.snap(r.origin.x + r.size.width * left);
        let x1 = self.snap(r.origin.x + r.size.width * right);
        let y0 = self.snap(r.origin.y + r.size.height * top);
        let y1 = self.snap(r.origin.y + r.size.height * bottom);
        CGContext::fill_rect(Some(ctx), rect(x0, y0, x1 - x0, y1 - y0));
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(width, height))
}

/// Draws `c` with the context's fill and stroke colour if it is a line or block character drawn by hand; whether it
/// was.
pub fn draw(ctx: &CGContext, c: char, pen: &Pen) -> bool {
    let code = u32::from(c);
    if let Some(dashes) = dashes(code) {
        dashed(ctx, dashes, pen);
        return true;
    }
    match code {
        0x2500..=0x254b => match LINES[usize::try_from(code - 0x2500).unwrap_or(0)] {
            Some(arms) => lines(ctx, arms, pen),
            None => return false,
        },
        0x2550..=0x256c => doubles(ctx, DOUBLES[usize::try_from(code - 0x2550).unwrap_or(0)], pen),
        0x2574..=0x257f => lines(ctx, HALVES[usize::try_from(code - 0x2574).unwrap_or(0)], pen),
        0x256d..=0x2570 => arc(ctx, code, pen),
        0x2571..=0x2573 => diagonals(ctx, code, pen),
        0x2580..=0x259f => block(ctx, code, pen),
        _ => return false,
    }
    true
}

/// Whether `draw` draws `c`.
pub fn covers(c: char) -> bool {
    let code = u32::from(c);
    dashes(code).is_some()
        || match code {
            0x2500..=0x254b => LINES[usize::try_from(code - 0x2500).unwrap_or(0)].is_some(),
            0x2550..=0x259f => true,
            _ => false,
        }
}

fn lines(ctx: &CGContext, arms: Arms, pen: &Pen) {
    let r = pen.rect;
    let width = |weight: u8| if weight == 2 { pen.light * 2.0 } else { pen.light };
    let widest = arms.iter().filter(|&&w| w > 0).map(|&w| width(w)).fold(0.0, f64::max);
    let center_x = r.origin.x + r.size.width / 2.0;
    let center_y = r.origin.y + r.size.height / 2.0;
    let (top, bottom) = (r.origin.y, r.origin.y + r.size.height);
    let (left, right) = (r.origin.x, r.origin.x + r.size.width);
    for (arm, &weight) in arms.iter().enumerate() {
        if weight == 0 {
            continue;
        }
        let thick = width(weight);
        let x = pen.snap(center_x - thick / 2.0);
        let y = pen.snap(center_y - thick / 2.0);
        let reach = widest / 2.0;
        let shape = match arm {
            0 => rect(x, top, thick, pen.snap(center_y + reach) - top),
            1 => {
                let from = pen.snap(center_x - reach);
                rect(from, y, right - from, thick)
            }
            2 => {
                let from = pen.snap(center_y - reach);
                rect(x, from, thick, bottom - from)
            }
            _ => rect(left, y, pen.snap(center_x + reach) - left, thick),
        };
        CGContext::fill_rect(Some(ctx), shape);
    }
}

/// Dashes spaced evenly, so that dashed lines run on across cells without a change in rhythm.
fn dashed(ctx: &CGContext, dashes: Dashes, pen: &Pen) {
    let r = pen.rect;
    let width = if dashes.heavy { pen.light * 2.0 } else { pen.light };
    let (start, length) = if dashes.across {
        (r.origin.x, r.size.width)
    } else {
        (r.origin.y, r.size.height)
    };
    let period = length / f64::from(dashes.count);
    let gap = (period * 0.4).max(1.0 / pen.scale);
    for n in 0..dashes.count {
        let from = start + period * f64::from(n) + gap / 2.0;
        let to = from + period - gap;
        if dashes.across {
            pen.across(ctx, from, to, r.origin.y + r.size.height / 2.0, width);
        } else {
            pen.down(ctx, from, to, r.origin.x + r.size.width / 2.0, width);
        }
    }
}

/// How far short of the middle a line of a double arm ends: at the near line of a double arm across from it (an
/// inner corner), past the far line of one (an outer corner), over a single line, or nowhere.
fn stop(near: u8, far: u8, gap: f64, width: f64) -> f64 {
    match (near, far) {
        (2, _) => gap - width / 2.0,
        (_, 2) => -(gap + width / 2.0),
        (1, _) | (_, 1) => -width / 2.0,
        _ => 0.0,
    }
}

/// How far past the middle a single arm reaches: through to the arm opposite it, to the near line of a double line
/// it meets from the side, or to the far line of a double corner it closes.
fn reach(opposite: u8, side: u8, other_side: u8, gap: f64, width: f64) -> f64 {
    if opposite > 0 {
        return width / 2.0;
    }
    match (side == 2, other_side == 2) {
        (true, true) => width / 2.0 - gap,
        (true, false) | (false, true) => gap + width / 2.0,
        (false, false) => width / 2.0,
    }
}

/// ═ ║ ╔ ╬ and their kin: double arms as two lines either side of the middle, single arms as one.
fn doubles(ctx: &CGContext, arms: Arms, pen: &Pen) {
    let [up, right, down, left] = arms;
    let r = pen.rect;
    let width = pen.light;
    let gap = width;
    let (center_x, center_y) = (r.origin.x + r.size.width / 2.0, r.origin.y + r.size.height / 2.0);
    let (top, bottom) = (r.origin.y, r.origin.y + r.size.height);
    let (left_edge, right_edge) = (r.origin.x, r.origin.x + r.size.width);
    for (offset, near, far) in [(-gap, up, down), (gap, down, up)] {
        let stop = stop(near, far, gap, width);
        if left == 2 {
            pen.across(ctx, left_edge, center_x - stop, center_y + offset, width);
        }
        if right == 2 {
            pen.across(ctx, center_x + stop, right_edge, center_y + offset, width);
        }
    }
    for (offset, near, far) in [(-gap, left, right), (gap, right, left)] {
        let stop = stop(near, far, gap, width);
        if up == 2 {
            pen.down(ctx, top, center_y - stop, center_x + offset, width);
        }
        if down == 2 {
            pen.down(ctx, center_y + stop, bottom, center_x + offset, width);
        }
    }
    if left == 1 {
        pen.across(
            ctx,
            left_edge,
            center_x + reach(right, up, down, gap, width),
            center_y,
            width,
        );
    }
    if right == 1 {
        pen.across(
            ctx,
            center_x - reach(left, up, down, gap, width),
            right_edge,
            center_y,
            width,
        );
    }
    if up == 1 {
        pen.down(
            ctx,
            top,
            center_y + reach(down, left, right, gap, width),
            center_x,
            width,
        );
    }
    if down == 1 {
        pen.down(
            ctx,
            center_y - reach(up, left, right, gap, width),
            bottom,
            center_x,
            width,
        );
    }
}

/// ╭ ╮ ╯ ╰: a quarter circle between the middle of two edges.
fn arc(ctx: &CGContext, code: u32, pen: &Pen) {
    let r = pen.rect;
    let center_x = pen.snap(r.origin.x + r.size.width / 2.0 - pen.light / 2.0) + pen.light / 2.0;
    let center_y = pen.snap(r.origin.y + r.size.height / 2.0 - pen.light / 2.0) + pen.light / 2.0;
    let (top, bottom) = (r.origin.y, r.origin.y + r.size.height);
    let (left, right) = (r.origin.x, r.origin.x + r.size.width);
    let (vertical_end, horizontal_end) = match code {
        0x256d => (bottom, right),
        0x256e => (bottom, left),
        0x256f => (top, left),
        _ => (top, right),
    };
    let radius = (r.size.width.min(r.size.height) / 2.0).max(1.0);
    let c = Some(ctx);
    CGContext::set_line_width(c, pen.light);
    CGContext::set_line_cap(c, CGLineCap::Butt);
    CGContext::begin_path(c);
    CGContext::move_to_point(c, center_x, vertical_end);
    CGContext::add_arc_to_point(c, center_x, center_y, horizontal_end, center_y, radius);
    CGContext::add_line_to_point(c, horizontal_end, center_y);
    CGContext::stroke_path(c);
}

fn diagonals(ctx: &CGContext, code: u32, pen: &Pen) {
    let r = pen.rect;
    let (top, bottom) = (r.origin.y, r.origin.y + r.size.height);
    let (left, right) = (r.origin.x, r.origin.x + r.size.width);
    let c = Some(ctx);
    CGContext::set_line_width(c, pen.light);
    CGContext::begin_path(c);
    if code != 0x2572 {
        CGContext::move_to_point(c, right, top);
        CGContext::add_line_to_point(c, left, bottom);
    }
    if code != 0x2571 {
        CGContext::move_to_point(c, left, top);
        CGContext::add_line_to_point(c, right, bottom);
    }
    CGContext::stroke_path(c);
}

fn block(ctx: &CGContext, code: u32, pen: &Pen) {
    let eighths = |n: u32| f64::from(n) / 8.0;
    match code {
        0x2580 => pen.fill(ctx, 0.0, 0.0, 1.0, 0.5),
        0x2581..=0x2588 => pen.fill(ctx, 0.0, 1.0 - eighths(code - 0x2580), 1.0, 1.0),
        0x2589..=0x258f => pen.fill(ctx, 0.0, 0.0, eighths(0x2590 - code), 1.0),
        0x2590 => pen.fill(ctx, 0.5, 0.0, 1.0, 1.0),
        0x2591..=0x2593 => {
            CGContext::save_g_state(Some(ctx));
            CGContext::set_alpha(Some(ctx), f64::from(code - 0x2590) / 4.0);
            pen.fill(ctx, 0.0, 0.0, 1.0, 1.0);
            CGContext::restore_g_state(Some(ctx));
        }
        0x2594 => pen.fill(ctx, 0.0, 0.0, 1.0, eighths(1)),
        0x2595 => pen.fill(ctx, eighths(7), 0.0, 1.0, 1.0),
        _ => {
            // Quadrants: upper left 1, upper right 2, lower left 4, lower right 8.
            const QUADRANTS: [u8; 10] = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14];
            let quadrants = QUADRANTS[usize::try_from(code - 0x2596).unwrap_or(0) % 10];
            for (bit, (left, top)) in [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)].into_iter().enumerate() {
                if quadrants & (1 << bit) != 0 {
                    pen.fill(ctx, left, top, left + 0.5, top + 0.5);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_lines_meet_at_inner_and_outer_corners() {
        let (gap, width) = (1.0, 1.0);
        // ╔: the upper line turns at the outer corner, the lower one at the inner.
        assert_eq!(stop(0, 2, gap, width), -1.5);
        assert_eq!(stop(2, 0, gap, width), 0.5);
        // ╟: the single arm ends at the near line; ╓: it reaches the far one; ╫: it crosses.
        assert_eq!(reach(0, 2, 2, gap, width), -0.5);
        assert_eq!(reach(0, 0, 2, gap, width), 1.5);
        assert_eq!(reach(1, 2, 2, gap, width), 0.5);
        assert_eq!(DOUBLES[0x256c - 0x2550], [2, 2, 2, 2]);
        assert_eq!(
            dashes(0x2508),
            Some(Dashes {
                across: true,
                heavy: false,
                count: 4
            })
        );
        assert_eq!(dashes(0x2550), None);
    }

    #[test]
    fn covers_names_exactly_the_characters_drawn_by_hand() {
        let space = objc2_core_graphics::CGColorSpace::new_device_rgb().unwrap();
        // SAFETY: A null buffer lets Core Graphics allocate one for the 8 × 16 pixels.
        let ctx =
            unsafe { objc2_core_graphics::CGBitmapContextCreate(std::ptr::null_mut(), 8, 16, 8, 0, Some(&space), 2) }
                .unwrap();
        let pen = Pen {
            rect: rect(0.0, 0.0, 8.0, 16.0),
            scale: 1.0,
            light: 1.0,
        };
        for c in ('\u{2400}'..='\u{25ff}').chain(['a', '漢', '─']) {
            assert_eq!(covers(c), draw(&ctx, c, &pen), "{c}");
        }
    }
}
