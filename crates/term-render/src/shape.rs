//! Ligatures, OpenType features and the axes of variable fonts (F13): the faces made with the settings' features,
//! and runs of text shaped by Core Text, whose glyphs are then put each in the cell of its character.

use std::ops::Range;
use std::ptr::NonNull;

use objc2_core_foundation::{
    CFArray, CFAttributedString, CFDictionary, CFIndex, CFNumber, CFRange, CFRetained, CFString, CFType, CGPoint,
};
use objc2_core_graphics::CGGlyph;
use term_session::{Cell, CellFlags, Point, Rgb};

use crate::palette::Theme;
use crate::render::{Canvas, Face, Fonts, Glyph};

use objc2_core_text::{
    CTFont, CTFontDescriptor, CTLine, CTRun, kCTFontAttributeName, kCTFontFeatureSettingsAttribute,
    kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTFontVariationAttribute,
};

/// `wght` as Core Text numbers an axis.
const WEIGHT: u32 = u32::from_be_bytes(*b"wght");

/// How the faces are set beyond family and size.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Typography {
    /// Text is shaped, so that ligatures and contextual alternates show.
    pub ligatures: bool,
    /// OpenType features by tag, 0 turning one off.
    pub features: Vec<(String, i32)>,
    /// Axes of a variable font by their number.
    pub variations: Vec<(u32, f64)>,
}

impl Typography {
    /// The typography the settings' `font` asks for.
    pub fn of(font: &term_config::Font) -> Typography {
        Typography {
            ligatures: font.ligatures,
            features: font.feature_settings(),
            variations: font.variation_settings(),
        }
    }

    /// Text has to be shaped: a feature only shows through shaping, not in the glyph a character maps to.
    pub fn shapes(&self) -> bool {
        self.ligatures || self.features.iter().any(|(_, value)| *value != 0)
    }
}

/// `font` with the features and axes of `typography`; a bold face keeps its own weight.
pub(crate) fn styled(font: CFRetained<CTFont>, typography: &Typography, bold: bool) -> CFRetained<CTFont> {
    let variations: Vec<_> = typography
        .variations
        .iter()
        .filter(|(axis, _)| !(bold && *axis == WEIGHT))
        .collect();
    if typography.features.is_empty() && variations.is_empty() {
        return font;
    }
    let mut keys: Vec<&CFString> = Vec::new();
    let mut features = None;
    let mut variation = None;
    if !typography.features.is_empty() {
        let settings: Vec<CFRetained<CFDictionary<CFString, CFType>>> = typography
            .features
            .iter()
            .map(|(tag, value)| {
                let tag = CFString::from_str(tag);
                let value = CFNumber::new_i32(*value);
                // SAFETY: Core Text exports its attribute names when it loads and never changes them.
                let names = unsafe { [kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue] };
                CFDictionary::from_slices(&names, &[&**tag, &**value])
            })
            .collect();
        let settings: Vec<&CFDictionary<CFString, CFType>> = settings.iter().map(|s| &**s).collect();
        // SAFETY: As above.
        keys.push(unsafe { kCTFontFeatureSettingsAttribute });
        features = Some(CFArray::from_objects(&settings));
    }
    if !variations.is_empty() {
        let axes: Vec<CFRetained<CFNumber>> = variations
            .iter()
            .map(|(axis, _)| CFNumber::new_i64(i64::from(*axis)))
            .collect();
        let amounts: Vec<CFRetained<CFNumber>> = variations.iter().map(|(_, v)| CFNumber::new_f64(*v)).collect();
        let axes: Vec<&CFNumber> = axes.iter().map(|a| &**a).collect();
        let amounts: Vec<&CFNumber> = amounts.iter().map(|a| &**a).collect();
        // SAFETY: As above.
        keys.push(unsafe { kCTFontVariationAttribute });
        variation = Some(CFDictionary::from_slices(&axes, &amounts));
    }
    let values: Vec<&CFType> = features
        .as_ref()
        .map(|f| -> &CFType { f.as_opaque() })
        .into_iter()
        .chain(variation.as_ref().map(|a| -> &CFType { a.as_opaque() }))
        .collect();
    let attributes = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // SAFETY: The dictionary holds font descriptor attributes with values of the types Core Text documents for them.
    let descriptor = unsafe { CTFontDescriptor::with_attributes(attributes.as_opaque()) };
    // SAFETY: Size 0 and a null matrix keep the font's own.
    unsafe { font.copy_with_attributes(0.0, std::ptr::null(), Some(&descriptor)) }
}

/// One glyph of a shaped run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shaped {
    /// The glyph in the font the run was shaped with.
    pub glyph: CGGlyph,
    /// The character it stands for, counted from the start of the run.
    pub index: usize,
    /// How far right of where its character's cell starts the glyph sits.
    pub offset: f64,
}

/// `text`, one character per cell `advance` wide, shaped in `font`; none where Core Text needs another font for any
/// of it, since then the cell-by-cell glyphs are as good.
pub(crate) fn shape(font: &CTFont, text: &str, advance: f64) -> Option<Vec<Shaped>> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let starts: Vec<usize> = text
        .chars()
        .scan(0, |at, c| {
            let start = *at;
            *at += c.len_utf16();
            Some(start)
        })
        .collect();
    let string = CFString::from_str(text);
    // SAFETY: Core Text exports its attribute names when it loads and never changes them.
    let font_key = unsafe { kCTFontAttributeName };
    let attributes = CFDictionary::<CFString, CFType>::from_slices(&[font_key], &[&**font]);
    // SAFETY: The string and attributes are valid for the call.
    let attributed = unsafe { CFAttributedString::new(None, Some(&string), Some(attributes.as_opaque())) }?;
    // SAFETY: As above.
    let line = unsafe { CTLine::with_attributed_string(&attributed) };
    // SAFETY: A line's glyph runs are an array of `CTRun`.
    let runs = unsafe { line.glyph_runs() };
    // SAFETY: As above.
    let runs: &CFArray<CTRun> = unsafe { runs.cast_unchecked() };
    let mut shaped = Vec::with_capacity(units.len());
    for run in runs {
        // SAFETY: The run belongs to the line, which is alive.
        let (count, run_attributes) = unsafe { (run.glyph_count(), run.attributes()) };
        // SAFETY: A run's attributes are a dictionary of string keys.
        let run_attributes: &CFDictionary<CFString, CFType> = unsafe { run_attributes.cast_unchecked() };
        let same_font = run_attributes.get(font_key).is_some_and(|used| {
            CFRetained::as_ptr(&used).cast::<CTFont>().as_ptr().cast_const() == std::ptr::from_ref(font)
        });
        if !same_font {
            return None;
        }
        let length = usize::try_from(count).ok()?;
        let mut glyphs: Vec<CGGlyph> = vec![0; length];
        let mut positions = vec![CGPoint::ZERO; length];
        let mut indices: Vec<CFIndex> = vec![0; length];
        let all = CFRange { location: 0, length: 0 };
        if length > 0 {
            // SAFETY: Each buffer holds the run's glyph count, which a zero-length range asks for.
            unsafe {
                run.glyphs(all, NonNull::new(glyphs.as_mut_ptr())?);
                run.positions(all, NonNull::new(positions.as_mut_ptr())?);
                run.string_indices(all, NonNull::new(indices.as_mut_ptr())?);
            }
        }
        for ((glyph, position), unit) in glyphs.into_iter().zip(positions).zip(indices) {
            let unit = usize::try_from(unit).ok()?;
            let index = starts.partition_point(|&start| start <= unit).checked_sub(1)?;
            if glyph == 0 {
                return None;
            }
            shaped.push(Shaped {
                glyph,
                index,
                offset: position.x - index as f64 * advance,
            });
        }
    }
    Some(shaped)
}

/// Where a run of shaped text goes.
pub(crate) struct Run {
    /// The screen line.
    pub(crate) line: usize,
    /// The baseline's height in the view.
    pub(crate) baseline: f64,
    /// The colour for all of it, else each cell's own.
    pub(crate) color: Option<Rgb>,
}

impl Run {
    /// Draws the run of cells that starts at `columns.start`, all in one face and of characters that shape with
    /// the terminal font, and gives the column after it; none where that first cell is none such.
    pub(crate) fn draw(
        &self,
        canvas: &mut impl Canvas,
        fonts: &Fonts,
        theme: &Theme,
        row: &[Cell],
        columns: Range<usize>,
    ) -> Option<usize> {
        let start = columns.start;
        let face = Face::of(shapable(row.get(start)?)?);
        let end = (start..columns.end.min(row.len()))
            .find(|&column| shapable(&row[column]).is_none_or(|flags| Face::of(flags) != face))
            .unwrap_or(columns.end.min(row.len()));
        let text: String = row[start..end].iter().map(|cell| cell.ch).collect();
        let shaped = fonts.shaped(&text, face)?;
        let m = fonts.metrics;
        for glyph in shaped.iter() {
            let column = start + glyph.index;
            let cell = &row[column];
            let fg = self.color.unwrap_or_else(|| theme.readable(cell.fg, cell.bg));
            let x =
                m.cell_rect(
                    Point {
                        line: self.line,
                        column,
                    },
                    1,
                )
                .origin
                .x + glyph.offset;
            let drawn = Glyph {
                font: face.index(),
                glyph: glyph.glyph,
                advance: m.cell_width,
            };
            canvas.glyph(drawn, x, self.baseline, fg);
        }
        Some(end)
    }
}

/// The flags of `cell` where it holds one printable ASCII character drawn in a terminal face; the rest goes cell by
/// cell, through the line drawing or the fallback fonts.
fn shapable(cell: &Cell) -> Option<CellFlags> {
    let plain = [CellFlags::WIDE, CellFlags::WIDE_SPACER, CellFlags::HIDDEN]
        .iter()
        .all(|&flag| !cell.flags.contains(flag));
    (plain && cell.zero_width.is_empty() && cell.ch.is_ascii_graphic()).then_some(cell.flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(name: &str) -> Option<CFRetained<CTFont>> {
        let name = CFString::from_str(name);
        // SAFETY: A null matrix keeps the font's own.
        let font = unsafe { CTFont::with_name(&name, 13.0, std::ptr::null()) };
        // SAFETY: A font always has a PostScript name.
        let found = unsafe { font.post_script_name() }.to_string();
        (found == name.to_string()).then_some(font)
    }

    fn advance(font: &CTFont) -> f64 {
        let shaped = shape(font, "MM", 0.0).unwrap();
        shaped[1].offset
    }

    #[test]
    fn plain_text_shapes_to_one_glyph_per_cell_without_offsets() {
        let menlo = font("Menlo-Regular").expect("Menlo ships with macOS");
        let advance = advance(&menlo);
        let shaped = shape(&menlo, "a->b", advance).unwrap();
        assert_eq!(shaped.iter().map(|s| s.index).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        assert!(shaped.iter().all(|s| s.offset.abs() < 0.01), "{shaped:?}");
    }

    #[test]
    fn a_ligature_font_swaps_the_glyphs_and_turning_calt_off_brings_them_back() {
        let Some(jetbrains) = font("JetBrainsMonoNFM-Regular") else {
            eprintln!("JetBrains Mono Nerd Font Mono is not installed; skipped");
            return;
        };
        let advance = advance(&jetbrains);
        let plain = shape(&jetbrains, "- >", advance).unwrap();
        let joined = shape(&jetbrains, "->", advance).unwrap();
        assert_ne!(joined[0].glyph, plain[0].glyph, "the arrow is a ligature");
        let off = Typography {
            features: vec![("calt".into(), 0)],
            ..Typography::default()
        };
        let off = styled(jetbrains.clone(), &off, false);
        let unjoined = shape(&off, "->", advance).unwrap();
        assert_eq!(unjoined[0].glyph, plain[0].glyph);
        assert_eq!(unjoined[1].glyph, plain[2].glyph);
    }
}
