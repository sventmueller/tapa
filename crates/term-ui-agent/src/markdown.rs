//! Agent answers as native text: splat's preview layout of the markdown, set as an attributed string for a TextKit 1
//! text view, which draws tables, code blocks and quotes as text blocks.

use std::collections::HashMap;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSColor, NSFont, NSFontManager, NSFontTraitMask, NSMutableParagraphStyle, NSTextAlignment, NSTextAttachment,
    NSTextBlock, NSTextBlockLayer, NSTextBlockValueType, NSTextTab, NSTextTable, NSTextTableBlock,
    NSTextTableLayoutAlgorithm, NSWritingDirection,
};
use objc2_foundation::{NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSRectEdge, NSURL};
use splat_markdown::code;
use splat_markdown::doc::Align;
use splat_markdown::highlight::Token;
use splat_markdown::style::Baseline;
use splat_preview::layout::{Atom, Link, NoImages, Para, ParaKind, ParaStyle, Run, RunMap, layout};
use splat_render::js;

use super::attrs::{Attribute, add_attribute, attributed, attributed_mut, utf16_range};
use super::style::{code_background, mono, token_color};
use crate::common::util::ns;

mod stream;
pub(crate) use stream::Streaming;

const LIST_INDENT: f64 = 22.0;
const QUOTE_INDENT: f64 = 12.0;

/// What the whole text is drawn in, before runs change it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    /// The agent's answer.
    Primary,
    /// Thinking and other quiet text.
    Secondary,
}

/// `markdown` as text at `size` points, its formulas and diagrams as code, as a plan to approve shows it.
pub(crate) fn render(mtm: MainThreadMarker, markdown: &str, size: f64, tone: Tone) -> Retained<NSAttributedString> {
    build(mtm, markdown, size, tone, None)
}

/// `markdown` as text at `size` points, `width` wide; its formulas and diagrams show as pictures once the shared
/// renderer drew them, as code until then.
pub(crate) fn render_with_figures(
    mtm: MainThreadMarker,
    markdown: &str,
    size: f64,
    tone: Tone,
    width: f64,
) -> Retained<NSAttributedString> {
    build(mtm, markdown, size, tone, Some(width))
}

fn build(
    mtm: MainThreadMarker,
    markdown: &str,
    size: f64,
    tone: Tone,
    figures: Option<f64>,
) -> Retained<NSAttributedString> {
    let doc = splat_markdown::reader::parse(markdown);
    let dark = super::figures::is_dark(mtm);
    let (laid, macros) = match figures {
        Some(_) => {
            let wanted = js::wanted(&doc, dark);
            let ready = js::Ready {
                dark,
                macros: wanted.macros.clone(),
            };
            super::figures::want_among(&wanted.keys, wanted.keys.clone());
            (layout(&doc, &ready, None), ready.macros)
        }
        None => (layout(&doc, &NoImages, None), Arc::from("")),
    };
    let renderer = Renderer {
        mtm,
        size,
        tone,
        dark,
        macros,
        width: figures.unwrap_or(0.0),
    };
    let paras: Vec<_> = laid
        .blocks
        .iter()
        .enumerate()
        .flat_map(|(n, block)| block.paras.iter().map(move |p| (n, p)))
        .collect();
    render_paragraphs(&renderer, &paras, true)
}

fn render_paragraphs(
    renderer: &Renderer,
    paras: &[(usize, &Para)],
    document_end: bool,
) -> Retained<NSAttributedString> {
    let out = NSMutableAttributedString::new();
    let mut frames = Frames::default();
    for (n, (block, para)) in paras.iter().enumerate() {
        let next = paras.get(n + 1);
        let last = next.is_none() && document_end;
        let continues =
            next.is_some_and(|(b, p)| b == block && p.style.group == para.style.group && para.style.group != 0);
        frames.block = *block;
        let style = renderer.paragraph_style(para, &mut frames, continues, last);
        for run in &para.runs {
            out.appendAttributedString(&renderer.run(run, para, &style));
        }
        // A table cell needs its newline, or it drops out of its table; the last paragraph of anything else would only
        // add an empty line below the text.
        if !last || matches!(para.style.kind, ParaKind::TableCell { .. }) {
            let newline = attributed(
                "\n",
                &[
                    Attribute::paragraph(&style),
                    Attribute::font(&renderer.base_font(&para.style.kind)),
                ],
            );
            out.appendAttributedString(&newline);
        }
    }
    Retained::into_super(out)
}

/// Plain text in one font and colour, for text that is not markdown, such as what the user typed; it runs in the
/// direction of its first letter, as a paragraph of an answer does.
pub(crate) fn plain(text: &str, font: &NSFont, color: &NSColor) -> Retained<NSAttributedString> {
    styled_plain(text, font, color, direction_of(text))
}

/// The direction of `text` by its first letter; natural, the interface's, without one. A natural paragraph takes the
/// interface's direction, also for a Latin text in a right-to-left interface.
fn direction_of(text: &str) -> NSWritingDirection {
    let right_to_left = |c: char| matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF);
    text.chars()
        .find(|c| c.is_alphabetic())
        .map_or(NSWritingDirection::Natural, |c| {
            if right_to_left(c) {
                NSWritingDirection::RightToLeft
            } else {
                NSWritingDirection::LeftToRight
            }
        })
}

/// Code or program output as `plain` sets it, left to right whatever the interface's direction.
pub(crate) fn code(text: &str, font: &NSFont, color: &NSColor) -> Retained<NSAttributedString> {
    styled_plain(text, font, color, NSWritingDirection::LeftToRight)
}

fn styled_plain(
    text: &str,
    font: &NSFont,
    color: &NSColor,
    direction: NSWritingDirection,
) -> Retained<NSAttributedString> {
    let style = NSMutableParagraphStyle::new();
    style.setLineHeightMultiple(1.1);
    style.setBaseWritingDirection(direction);
    attributed(
        text,
        &[
            Attribute::font(font),
            Attribute::foreground(color),
            Attribute::paragraph(&style),
        ],
    )
}

#[derive(Default)]
struct Frames {
    /// The top-level block the paragraphs come from now.
    block: usize,
    groups: HashMap<(usize, usize), Retained<NSTextBlock>>,
    tables: HashMap<(usize, u32), Retained<NSTextTable>>,
}

struct Renderer {
    mtm: MainThreadMarker,
    size: f64,
    tone: Tone,
    dark: bool,
    macros: Arc<str>,
    width: f64,
}

impl Renderer {
    fn base_font(&self, kind: &ParaKind) -> Retained<NSFont> {
        let s = self.size;
        match kind {
            ParaKind::Heading(level) => {
                let factor = [1.4, 1.22, 1.1, 1.0, 1.0, 0.95][usize::from(*level).clamp(1, 6) - 1];
                super::style::bold(s * factor)
            }
            ParaKind::Code { .. } | ParaKind::MathBlock | ParaKind::Diagram => mono(s * 0.92),
            ParaKind::FootnoteDef | ParaKind::FrontMatter => NSFont::systemFontOfSize(s * 0.9),
            ParaKind::AlertTitle => super::style::bold(s),
            ParaKind::TableCell { header: true, .. } => super::style::bold(s),
            _ => NSFont::systemFontOfSize(s),
        }
    }

    fn color(&self, run: &Run, para: &Para) -> Retained<NSColor> {
        if self.tone == Tone::Secondary {
            return NSColor::secondaryLabelColor();
        }
        if matches!(run.map, RunMap::Atom { atom: Atom::Marker, .. })
            || para.style.quote_depth > 0
            || matches!(para.style.kind, ParaKind::FootnoteDef | ParaKind::FrontMatter)
            || run.style.html
        {
            return NSColor::secondaryLabelColor();
        }
        if run.style.link.is_some() {
            return NSColor::linkColor();
        }
        NSColor::labelColor()
    }

    fn paragraph_style(
        &self,
        para: &Para,
        frames: &mut Frames,
        continues: bool,
        last: bool,
    ) -> Retained<NSMutableParagraphStyle> {
        let s = self.size;
        let style = &para.style;
        let st = NSMutableParagraphStyle::new();
        let framed_alone = matches!(style.kind, ParaKind::Code { .. } | ParaKind::TableCell { .. });
        let indent = s / super::style::BODY;
        let list = f64::from(style.list_depth) * LIST_INDENT * indent;
        let quote = if framed_alone {
            0.0
        } else {
            f64::from(style.quote_depth) * QUOTE_INDENT * indent
        };
        let head = list + quote;
        st.setBaseWritingDirection(match style.rtl {
            Some(true) => NSWritingDirection::RightToLeft,
            Some(false) => NSWritingDirection::LeftToRight,
            None => NSWritingDirection::Natural,
        });
        st.setHeadIndent(head);
        st.setFirstLineHeadIndent(if style.marker.is_some() {
            head - LIST_INDENT * indent * 0.85
        } else {
            head
        });
        if style.marker.is_some() {
            st.setTabStops(Some(&NSArray::from_slice(&[&*tab_stop(head)])));
        }
        st.setLineHeightMultiple(1.15);
        let spacing = if style.tight && style.list_depth > 0 {
            s * 0.2
        } else {
            s * 0.55
        };
        st.setParagraphSpacing(if last { 0.0 } else { spacing });
        if style.space_before {
            st.setParagraphSpacingBefore(s * 0.6);
        }
        self.frame(para, &st, frames, continues, last);
        if !matches!(style.kind, ParaKind::TableCell { .. }) {
            match style.align {
                Align::Center => st.setAlignment(NSTextAlignment::Center),
                Align::Right => st.setAlignment(NSTextAlignment::Right),
                Align::Left => st.setAlignment(NSTextAlignment::Left),
                Align::None => {}
            }
        }
        st
    }

    /// The text block a paragraph stands in, and what changes with it.
    fn frame(&self, para: &Para, st: &NSMutableParagraphStyle, frames: &mut Frames, continues: bool, last: bool) {
        let s = self.size;
        let style = &para.style;
        match &style.kind {
            ParaKind::Heading(_) => {
                st.setParagraphSpacingBefore(s * 0.7);
                st.setParagraphSpacing(if last { 0.0 } else { s * 0.35 });
            }
            ParaKind::Code { .. } | ParaKind::MathBlock | ParaKind::Diagram => {
                // Code reads left to right whatever the language of the interface.
                st.setBaseWritingDirection(NSWritingDirection::LeftToRight);
                st.setParagraphSpacing(0.0);
                st.setLineHeightMultiple(1.05);
                let block = frames
                    .groups
                    .entry((frames.block, style.group))
                    .or_insert_with(|| code_frame(s, last && !continues));
                st.setTextBlocks(&NSArray::from_slice(&[&**block]));
            }
            ParaKind::TableCell {
                table,
                row,
                col,
                header,
                columns,
                span,
                align,
            } => {
                st.setParagraphSpacing(0.0);
                st.setHeadIndent(0.0);
                st.setFirstLineHeadIndent(0.0);
                st.setAlignment(match align {
                    Align::Left | Align::None => NSTextAlignment::Natural,
                    Align::Center => NSTextAlignment::Center,
                    Align::Right => NSTextAlignment::Right,
                });
                let t = frames
                    .tables
                    .entry((frames.block, *table))
                    .or_insert_with(|| text_table(s, *columns as usize));
                let span = (*span).max(1);
                let column = if style.mirrored {
                    columns.saturating_sub(col + span)
                } else {
                    *col
                };
                let cell = table_cell(t, s, (*row as usize, column as usize, span as usize), *header);
                st.setTextBlocks(&NSArray::from_slice(&[&*cell as &NSTextBlock]));
            }
            ParaKind::Rule => {
                st.setLineHeightMultiple(0.5);
                st.setTextBlocks(&NSArray::from_slice(&[&*rule_frame(s)]));
            }
            _ if style.quote_depth > 0 => {
                let block = frames
                    .groups
                    .entry((frames.block, style.group))
                    .or_insert_with(|| quote_frame(style, s));
                st.setTextBlocks(&NSArray::from_slice(&[&**block]));
                st.setParagraphSpacing(if continues { s * 0.4 } else { 0.0 });
            }
            _ => {}
        }
    }

    fn font(&self, run: &Run, para: &Para) -> Retained<NSFont> {
        let base = self.base_font(&para.style.kind);
        let mut font = if run.style.text.code || run.style.html {
            mono(base.pointSize() * 0.92)
        } else {
            base
        };
        if run.style.text.baseline != Baseline::Normal {
            font = NSFont::fontWithDescriptor_size(&font.fontDescriptor(), font.pointSize() * 0.72).unwrap_or(font);
        }
        let manager = NSFontManager::sharedFontManager(self.mtm);
        if run.style.text.bold {
            font = manager.convertFont_toHaveTrait(&font, NSFontTraitMask::BoldFontMask);
        }
        if run.style.text.italic {
            font = manager.convertFont_toHaveTrait(&font, NSFontTraitMask::ItalicFontMask);
        }
        font
    }

    /// The picture the shared renderer drew of the formula or diagram `run` stands for.
    fn figure(&self, run: &Run, font: &NSFont) -> Option<Retained<NSTextAttachment>> {
        let key = match &run.map {
            RunMap::Atom {
                atom: Atom::Math { tex, display },
                ..
            } => js::Key::math(tex, *display, self.dark, &self.macros),
            RunMap::Atom {
                atom: Atom::Diagram { source, kind },
                ..
            } => js::Key::diagram(source, *kind, self.dark),
            _ => return None,
        };
        let rendered = js::cached(&key)?;
        Some(splat_render::render::attachment(&rendered, font, self.width))
    }

    fn run(&self, run: &Run, para: &Para, paragraph: &NSMutableParagraphStyle) -> Retained<NSAttributedString> {
        let font = self.font(run, para);
        let mut attributes = vec![
            Attribute::font(&font),
            Attribute::foreground(&self.color(run, para)),
            Attribute::paragraph(paragraph),
        ];
        if let Some(attachment) = self.figure(run, &font) {
            attributes.push(Attribute::attachment(&attachment));
            return attributed("\u{FFFC}", &attributes);
        }
        if run.style.text.strike {
            attributes.push(Attribute::strikethrough());
        }
        if run.style.text.underline {
            attributes.push(Attribute::underline());
        }
        let in_code_block = matches!(para.style.kind, ParaKind::Code { .. });
        if (run.style.text.code && !in_code_block) || run.style.text.highlight {
            attributes.push(Attribute::background(&code_background()));
        }
        match run.style.text.baseline {
            Baseline::Super => attributes.push(Attribute::baseline_offset(font.pointSize() * 0.55)),
            Baseline::Sub => attributes.push(Attribute::baseline_offset(font.pointSize() * -0.25)),
            Baseline::Normal => {}
        }
        if let Some(Link::Address(address)) = &run.style.link
            && let Some(url) = NSURL::URLWithString(&ns(address))
        {
            attributes.push(Attribute::link(&url));
            attributes.push(Attribute::tool_tip(address));
        }
        let text = shown_text(run);
        let out = attributed_mut(&text, &attributes);
        if let (ParaKind::Code { info }, RunMap::Text { .. }, Tone::Primary) = (&para.style.kind, &run.map, self.tone) {
            for (range, token) in code::tokens(info, &text) {
                add_attribute(
                    &out,
                    &Attribute::foreground(&token_color(Token::from(token))),
                    utf16_range(&text, range.start, range.end, 0),
                );
            }
        }
        Retained::into_super(out)
    }
}

/// What a run shows: atoms that would be pictures show their text instead.
fn shown_text(run: &Run) -> String {
    match &run.map {
        RunMap::Atom {
            atom: Atom::Image(image),
            ..
        } => format!("[{}]", image.alt),
        RunMap::Atom {
            atom: Atom::Math { tex, .. },
            ..
        } => tex.to_string(),
        RunMap::Atom {
            atom: Atom::Fold { .. } | Atom::Details { .. } | Atom::AlertIcon(_),
            ..
        } => String::new(),
        _ => run.text.replace(['\u{FFFC}'], "").replace('\u{2028}', "\n"),
    }
}

fn tab_stop(location: f64) -> Retained<NSTextTab> {
    // SAFETY: Empty options, which NSTextTab accepts.
    unsafe {
        NSTextTab::initWithTextAlignment_location_options(
            NSTextTab::alloc(),
            NSTextAlignment::Natural,
            location,
            &NSDictionary::new(),
        )
    }
}

/// A text block as wide as the text view; standalone blocks have no width unless one is set.
fn full_width_block() -> Retained<NSTextBlock> {
    let b = NSTextBlock::new();
    b.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
    b
}

fn set_edge(block: &NSTextBlock, width: f64, layer: NSTextBlockLayer, edge: NSRectEdge) {
    block.setWidth_type_forLayer_edge(width, NSTextBlockValueType::AbsoluteValueType, layer, edge);
}

fn code_frame(s: f64, last: bool) -> Retained<NSTextBlock> {
    let b = full_width_block();
    b.setBackgroundColor(Some(&code_background()));
    b.setWidth_type_forLayer(
        s * 0.6,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
    );
    if !last {
        set_edge(&b, s * 0.6, NSTextBlockLayer::Margin, NSRectEdge::MaxY);
    }
    b
}

fn rule_frame(s: f64) -> Retained<NSTextBlock> {
    let b = full_width_block();
    set_edge(&b, 1.0, NSTextBlockLayer::Border, NSRectEdge::MaxY);
    b.setBorderColor_forEdge(Some(&NSColor::separatorColor()), NSRectEdge::MaxY);
    set_edge(&b, s * 0.5, NSTextBlockLayer::Margin, NSRectEdge::MaxY);
    b
}

/// The bar beside a quote, on the side its text starts.
fn quote_frame(style: &ParaStyle, s: f64) -> Retained<NSTextBlock> {
    let b = full_width_block();
    let edge = if style.mirrored {
        NSRectEdge::MaxX
    } else {
        NSRectEdge::MinX
    };
    set_edge(&b, 3.0, NSTextBlockLayer::Border, edge);
    b.setBorderColor_forEdge(Some(&NSColor::tertiaryLabelColor()), edge);
    set_edge(&b, s * 0.7, NSTextBlockLayer::Padding, edge);
    set_edge(&b, s * 0.5, NSTextBlockLayer::Margin, NSRectEdge::MaxY);
    b
}

fn text_table(s: f64, columns: usize) -> Retained<NSTextTable> {
    let t = NSTextTable::new();
    t.setContentWidth_type(100.0, NSTextBlockValueType::PercentageValueType);
    t.setNumberOfColumns(columns);
    t.setLayoutAlgorithm(NSTextTableLayoutAlgorithm::AutomaticLayoutAlgorithm);
    t.setCollapsesBorders(true);
    t.setHidesEmptyCells(false);
    set_edge(&t, s * 0.6, NSTextBlockLayer::Margin, NSRectEdge::MaxY);
    t
}

fn table_cell(
    table: &NSTextTable,
    s: f64,
    (row, column, span): (usize, usize, usize),
    header: bool,
) -> Retained<NSTextTableBlock> {
    let to_isize = |n: usize| isize::try_from(n).unwrap_or(isize::MAX);
    let cell = NSTextTableBlock::initWithTable_startingRow_rowSpan_startingColumn_columnSpan(
        NSTextTableBlock::alloc(),
        table,
        to_isize(row),
        1,
        to_isize(column),
        to_isize(span),
    );
    cell.setWidth_type_forLayer(1.0, NSTextBlockValueType::AbsoluteValueType, NSTextBlockLayer::Border);
    cell.setBorderColor(Some(&NSColor::separatorColor()));
    cell.setWidth_type_forLayer(
        s * 0.4,
        NSTextBlockValueType::AbsoluteValueType,
        NSTextBlockLayer::Padding,
    );
    if header {
        cell.setBackgroundColor(Some(&code_background()));
    }
    cell
}

#[cfg(test)]
mod tests {

    #[test]
    fn plain_text_runs_in_the_direction_of_its_first_letter() {
        use objc2_app_kit::NSWritingDirection;
        assert_eq!(
            super::direction_of("  1) Run the tests"),
            NSWritingDirection::LeftToRight
        );
        assert_eq!(super::direction_of("— مرحبا, world"), NSWritingDirection::RightToLeft);
        assert_eq!(super::direction_of("שלום"), NSWritingDirection::RightToLeft);
        assert_eq!(super::direction_of("42 + 7"), NSWritingDirection::Natural);
    }

    use super::*;

    #[test]
    fn markers_and_pictures_show_as_text() {
        let doc = splat_markdown::reader::parse("- one\n\n![a cat](cat.png)\n");
        let laid = layout(&doc, &NoImages, None);
        let texts: Vec<String> = laid
            .blocks
            .iter()
            .flat_map(|b| b.paras.iter())
            .flat_map(|p| p.runs.iter())
            .map(shown_text)
            .collect();
        assert!(texts.iter().any(|t| t == "[a cat]"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("one")), "{texts:?}");
    }
}
