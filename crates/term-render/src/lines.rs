//! The quads of each line kept from frame to frame (E5): a line whose cells, selection and background did not change
//! gives its quads again, so a frame costs the processor only the lines that changed and the cursor. Everything is
//! made anew when the fonts, the theme, the size or the atlas change.

use objc2_core_foundation::CGRect;
use term_session::{Cell, Rgb, Snapshot};

use crate::palette::Theme;
use crate::render::{self, Canvas, Fonts, Overlay};
use crate::sprites::{Quad, QuadCanvas, Sprites};

#[derive(Debug, Clone, PartialEq)]
struct Key {
    fonts: u64,
    atlas: u64,
    theme: Theme,
    width: u64,
    height: u64,
}

#[derive(Debug)]
struct Line {
    cells: Vec<Cell>,
    selection: Option<(usize, usize)>,
    background: Rgb,
    quads: Vec<Quad>,
}

/// The kept quads of a surface's lines.
#[derive(Debug, Default)]
pub(crate) struct Lines {
    key: Option<Key>,
    lines: Vec<Option<Line>>,
    /// How many lines the last frame painted anew, for the bench.
    pub(crate) painted: usize,
}

impl Lines {
    /// The quads of a frame of `screen` with `overlay` over it.
    pub(crate) fn quads(
        &mut self,
        sprites: &mut Sprites,
        fonts: &Fonts,
        theme: &Theme,
        screen: &Snapshot,
        overlay: &Overlay,
        bounds: CGRect,
    ) -> Vec<Quad> {
        loop {
            sprites.prepare(fonts);
            let key = Key {
                fonts: fonts.id,
                atlas: sprites.generation(),
                theme: theme.clone(),
                width: bounds.size.width.to_bits(),
                height: bounds.size.height.to_bits(),
            };
            if self.key.as_ref() != Some(&key) {
                self.key = Some(key);
                self.lines.clear();
            }
            self.lines.resize_with(screen.lines, || None);
            let quads = self.frame(sprites, fonts, theme, screen, overlay, bounds);
            if !sprites.full() || !sprites.make_room() {
                return quads;
            }
            self.lines.clear();
        }
    }

    fn frame(
        &mut self,
        sprites: &mut Sprites,
        fonts: &Fonts,
        theme: &Theme,
        screen: &Snapshot,
        overlay: &Overlay,
        bounds: CGRect,
    ) -> Vec<Quad> {
        let mut quads = Vec::with_capacity(screen.columns * screen.lines * 2);
        let mut canvas = QuadCanvas::new(sprites, fonts);
        canvas.fill(screen.background, 1.0, bounds);
        quads.append(&mut canvas.quads);
        drop(canvas);
        self.painted = 0;
        let width = bounds.origin.x + bounds.size.width;
        for (index, slot) in self.lines.iter_mut().enumerate() {
            let Some(cells) = screen.rows.get(index) else { continue };
            let selection = render::selected_columns(screen, index);
            if let Some(kept) = slot
                && kept.selection == selection
                && kept.background == screen.background
                && kept.cells == *cells
            {
                quads.extend_from_slice(&kept.quads);
                continue;
            }
            let mut canvas = QuadCanvas::new(sprites, fonts);
            render::paint_line(&mut canvas, fonts, theme, screen, index, width);
            quads.extend_from_slice(&canvas.quads);
            *slot = Some(Line {
                cells: cells.clone(),
                selection,
                background: screen.background,
                quads: std::mem::take(&mut canvas.quads),
            });
            self.painted += 1;
        }
        let mut canvas = QuadCanvas::new(sprites, fonts);
        render::paint_overlay(&mut canvas, fonts, theme, screen, overlay, bounds);
        quads.append(&mut canvas.quads);
        quads
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use term_session::{CellFlags, Cursor, CursorShape, Damage, InputModes, Point};

    use super::*;

    fn screen(columns: usize, lines: usize, marker: char) -> Snapshot {
        let theme = crate::palette::theme(true, false);
        let cell = |ch| Cell {
            ch,
            zero_width: Vec::new(),
            fg: theme.palette.foreground,
            bg: theme.palette.background,
            underline_color: None,
            flags: CellFlags::default(),
            hyperlink: None,
        };
        let mut rows: Vec<Vec<Cell>> = (0..lines)
            .map(|line| {
                (0..columns)
                    .map(|column| cell(char::from(b'a' + u8::try_from((line + column) % 26).unwrap_or(0))))
                    .collect()
            })
            .collect();
        rows[0][0] = cell(marker);
        Snapshot {
            columns,
            lines,
            rows,
            cursor: Cursor {
                point: Point { line: 0, column: 0 },
                shape: CursorShape::Block,
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

    #[test]
    fn only_lines_that_changed_are_painted_again_and_it_pays() {
        let Some(gpu) = crate::gpu::shared() else { return };
        let Some(mut sprites) = Sprites::new(gpu.device()) else {
            return;
        };
        let fonts = Fonts::new(13.0, 2.0);
        let theme = crate::palette::theme(true, false);
        let overlay = Overlay {
            focused: true,
            cursor_on: true,
            hide_cursor: false,
            marked: "",
            final_line: None,
            flash: false,
        };
        let bounds = render::rect(0.0, 0.0, 1800.0, 1100.0);
        let mut lines = Lines::default();
        let first = screen(200, 60, 'x');
        let started = Instant::now();
        let full = lines.quads(&mut sprites, &fonts, &theme, &first, &overlay, bounds);
        let full_time = started.elapsed();
        sprites.uploaded();
        assert_eq!(lines.painted, 60);
        let started = Instant::now();
        let again = lines.quads(&mut sprites, &fonts, &theme, &first, &overlay, bounds);
        let kept_time = started.elapsed();
        assert_eq!(lines.painted, 0);
        assert_eq!(again, full, "kept quads are the quads");
        lines.quads(&mut sprites, &fonts, &theme, &screen(200, 60, 'y'), &overlay, bounds);
        assert_eq!(lines.painted, 1);
        println!("a frame of 200 x 60: {full_time:?} painted, {kept_time:?} kept");

        let typography = crate::shape::Typography {
            ligatures: true,
            ..Default::default()
        };
        let shaping = Fonts::styled("", 13.0, 2.0, &typography);
        let mut lines = Lines::default();
        let started = Instant::now();
        lines.quads(&mut sprites, &shaping, &theme, &first, &overlay, bounds);
        let shaped_time = started.elapsed();
        assert_eq!(lines.painted, 60);
        let plain = Fonts::new(13.0, 2.0);
        let mut lines = Lines::default();
        let started = Instant::now();
        lines.quads(&mut sprites, &plain, &theme, &first, &overlay, bounds);
        println!(
            "the same frame shaped: {shaped_time:?}, unshaped again: {:?}",
            started.elapsed()
        );
    }
}
