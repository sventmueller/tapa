//! The drawing of a screen: selections, cursor shapes, the line after the program ended and the bell's flash.

use super::*;

#[test]
fn selections_cover_the_right_columns_of_each_line() {
    let at = |line, column| Point { line, column };
    let flowing = Highlight {
        start: at(1, 5),
        end: at(3, 2),
        block: false,
    };
    assert_eq!(selected(&flowing, 0, 80), None);
    assert_eq!(selected(&flowing, 1, 80), Some((5, 79)));
    assert_eq!(selected(&flowing, 2, 80), Some((0, 79)));
    assert_eq!(selected(&flowing, 3, 80), Some((0, 2)));
    let block = Highlight {
        start: at(1, 9),
        end: at(2, 4),
        block: true,
    };
    assert_eq!(selected(&block, 2, 80), Some((4, 9)));
}

use crate::{palette, sprites};
use term_session::{Cursor, Damage, InputModes};

const SCALE: f64 = 2.0;

/// Six blank columns on three lines, with the cursor in the middle of the second.
fn blank(theme: &Theme, shape: CursorShape) -> Snapshot {
    let empty = Cell {
        ch: ' ',
        zero_width: vec![],
        fg: theme.palette.foreground,
        bg: theme.palette.background,
        underline_color: None,
        flags: CellFlags::default(),
        hyperlink: None,
    };
    Snapshot {
        columns: 6,
        lines: 3,
        rows: vec![vec![empty; 6]; 3],
        cursor: Cursor {
            point: Point { line: 1, column: 2 },
            shape,
            visible: true,
            blinking: false,
            wide: false,
        },
        selection: None,
        display_offset: 0,
        history: 0,
        damage: Damage::Full,
        background: theme.palette.background,
        modes: InputModes::default(),
    }
}

/// The screen drawn with Core Graphics: its pixels and how many there are in a row.
fn drawn(fonts: &Fonts, theme: &Theme, screen: &Snapshot, overlay: &Overlay) -> (Vec<u8>, usize) {
    let m = fonts.metrics;
    let bounds = rect(
        0.0,
        0.0,
        2.0 * INSET_X + 6.0 * m.cell_width,
        2.0 * INSET_Y + 3.0 * m.cell_height,
    );
    let width = clamp_to(bounds.size.width * SCALE, 4096);
    let height = clamp_to(bounds.size.height * SCALE, 4096);
    let pixels = sprites::bitmap(width, height, SCALE, (0.0, 0.0), &|ctx| {
        draw(ctx, fonts, theme, screen, overlay, bounds);
    })
    .unwrap();
    (pixels, width)
}

/// The pixels of `area` that differ from the top left pixel, which is the background.
fn inked(pixels: &[u8], width: usize, area: CGRect) -> Vec<(usize, usize)> {
    let background = &pixels[0..4];
    let (left, top) = (
        clamp_to(area.origin.x * SCALE, width),
        clamp_to(area.origin.y * SCALE, 4096),
    );
    let right = clamp_to((area.origin.x + area.size.width) * SCALE, width);
    let bottom = clamp_to((area.origin.y + area.size.height) * SCALE, pixels.len() / 4 / width);
    let mut found = Vec::new();
    for y in top..bottom {
        for x in left..right {
            let at = (y * width + x) * 4;
            if pixels[at..at + 3]
                .iter()
                .zip(background)
                .any(|(a, b)| a.abs_diff(*b) > 24)
            {
                found.push((x - left, y - top));
            }
        }
    }
    found
}

fn overlay<'a>() -> Overlay<'a> {
    Overlay {
        focused: true,
        cursor_on: true,
        hide_cursor: false,
        marked: "",
        final_line: None,
        flash: false,
    }
}

#[test]
fn each_cursor_shape_covers_its_part_of_the_cell() {
    let fonts = Fonts::new(13.0, SCALE);
    let theme = palette::theme(true, true);
    let m = fonts.metrics;
    let cell = m.cell_rect(Point { line: 1, column: 2 }, 1);
    let (w, h) = (
        clamp_to(cell.size.width * SCALE, 4096),
        clamp_to(cell.size.height * SCALE, 4096),
    );
    let ink = |shape, overlay: &Overlay| {
        let (pixels, width) = drawn(&fonts, &theme, &blank(&theme, shape), overlay);
        (
            inked(&pixels, width, cell),
            inked(&pixels, width, rect(0.0, 0.0, 400.0, cell.origin.y)),
        )
    };
    let (block, above) = ink(CursorShape::Block, &overlay());
    assert!(
        block.len() * 10 >= w * h * 9 && above.is_empty(),
        "{} of {}",
        block.len(),
        w * h
    );
    let (beam, _) = ink(CursorShape::Beam, &overlay());
    assert!(!beam.is_empty() && beam.iter().all(|&(x, _)| x < w / 3), "{beam:?}");
    assert!(beam.iter().any(|&(_, y)| y < h / 4) && beam.iter().any(|&(_, y)| y > h * 3 / 4));
    let (line, _) = ink(CursorShape::Underline, &overlay());
    assert!(!line.is_empty() && line.iter().all(|&(_, y)| y > h * 3 / 4), "{line:?}");
    assert!(line.iter().any(|&(x, _)| x < w / 4) && line.iter().any(|&(x, _)| x > w * 3 / 4));
    let unfocused = Overlay {
        focused: false,
        ..overlay()
    };
    let (hollow, _) = ink(CursorShape::Block, &unfocused);
    assert!(
        !hollow.is_empty() && !hollow.contains(&(w / 2, h / 2)),
        "{} pixels",
        hollow.len()
    );
    let blinked_off = Overlay {
        cursor_on: false,
        ..overlay()
    };
    assert!(ink(CursorShape::Block, &blinked_off).0.is_empty());
    let stood_in_for = Overlay {
        hide_cursor: true,
        focused: false,
        ..overlay()
    };
    assert!(ink(CursorShape::Block, &stood_in_for).0.is_empty());
}

#[test]
fn the_end_of_the_program_is_written_instead_of_the_cursor_and_the_bell_flashes_the_whole_screen() {
    let fonts = Fonts::new(13.0, SCALE);
    let theme = palette::theme(false, true);
    let m = fonts.metrics;
    let screen = blank(&theme, CursorShape::Block);
    let ended = Overlay {
        final_line: Some((2, "[ended]")),
        ..overlay()
    };
    let (pixels, width) = drawn(&fonts, &theme, &screen, &ended);
    assert!(inked(&pixels, width, m.cell_rect(Point { line: 1, column: 2 }, 1)).is_empty());
    assert!(!inked(&pixels, width, m.line_rect(2, 400.0)).is_empty());
    let flashing = Overlay {
        flash: true,
        ..overlay()
    };
    let (plain, _) = drawn(&fonts, &theme, &screen, &overlay());
    let (flashed, _) = drawn(&fonts, &theme, &screen, &flashing);
    let corner = |pixels: &[u8]| pixels[0..3].to_vec();
    assert_ne!(
        corner(&plain),
        corner(&flashed),
        "the background changes everywhere, the corner too"
    );
}

#[test]
fn a_fill_paints_the_area_in_the_colour() {
    let red = Rgb::new(255, 0, 0);
    let pixels = sprites::bitmap(8, 8, 1.0, (0.0, 0.0), &|ctx| fill(ctx, red, rect(0.0, 0.0, 4.0, 8.0))).unwrap();
    let at = |x: usize| pixels[x * 4..x * 4 + 4].to_vec();
    assert_ne!(at(0), at(7));
    assert!(
        at(0).contains(&255) && at(0).iter().filter(|&&v| v == 0).count() >= 2,
        "{:?}",
        at(0)
    );
}

#[test]
fn counts_from_points_stay_in_range() {
    assert_eq!(clamp_to(-3.0, 10), 0);
    assert_eq!(clamp_to(4.7, 10), 4);
    assert_eq!(clamp_to(99.0, 10), 10);
    assert_eq!(clamp_to(f64::NAN, 10), 0);
}
