//! The suggestion for the line being typed (E7, F94), drawn faint from the cursor on while the cursor is still where
//! it was made for.

use term_session::{CursorShape, Snapshot};

use crate::palette::Theme;
use crate::render::{Canvas, Face, Fonts, Overlay};

/// How far the suggestion's colour lies from the text's toward the background.
pub const SHARE: f64 = 0.55;

/// The suggestion for the line being typed, from the cursor on; its first character in the background's colour where
/// a block cursor covers it, as the character under the cursor is drawn.
pub(crate) fn paint(canvas: &mut impl Canvas, fonts: &Fonts, theme: &Theme, screen: &Snapshot, overlay: &Overlay) {
    let Some((point, text)) = overlay.ghost else { return };
    if text.is_empty() || screen.display_offset != 0 || screen.cursor.point != point {
        return;
    }
    let m = fonts.metrics;
    let area = m.cell_rect(point, 1);
    let baseline = area.origin.y + m.baseline;
    let faint = theme.palette.foreground.mix(screen.background, SHARE);
    let at = screen.cursor;
    let covered =
        at.visible && !overlay.hide_cursor && overlay.focused && overlay.cursor_on && at.shape == CursorShape::Block;
    let Some(first) = text.chars().next().filter(|_| covered) else {
        canvas.line(text, Face::Regular, area.origin.x, baseline, faint);
        return;
    };
    let split = first.len_utf8();
    canvas.line(
        &text[..split],
        Face::Regular,
        area.origin.x,
        baseline,
        screen.background,
    );
    canvas.line(
        &text[split..],
        Face::Regular,
        area.origin.x + m.cell_width,
        baseline,
        faint,
    );
}
