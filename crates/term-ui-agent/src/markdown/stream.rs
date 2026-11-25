//! Incremental parsing and rendered-block reuse for the answer that is still arriving.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_foundation::{NSAttributedString, NSMutableAttributedString};
use splat_markdown::doc::Doc;
use splat_preview::layout::{Layout, layout, relayout, reusable};
use splat_render::js;

use super::{Renderer, Tone, render_paragraphs};

mod figures;

struct Block {
    key: u64,
    last: bool,
    text: Retained<NSAttributedString>,
}

/// The parsed answer and its rendered blocks; owned only by its visible text view.
pub(crate) struct Streaming {
    doc: Doc,
    laid: Layout,
    blocks: Vec<Block>,
    figures: figures::Figures,
    dark: bool,
    macros: std::sync::Arc<str>,
    width: f64,
    /// The text size the blocks were rendered at.
    size: f64,
}

impl Streaming {
    /// The answer `markdown` so far, parsed and rendered `width` wide, with its text.
    pub(crate) fn start(
        mtm: MainThreadMarker,
        markdown: &str,
        width: f64,
    ) -> (Streaming, Retained<NSAttributedString>) {
        let mut streaming = Streaming {
            doc: splat_markdown::reader::parse(markdown),
            laid: Layout::default(),
            blocks: Vec::new(),
            figures: figures::Figures::default(),
            dark: false,
            macros: "".into(),
            width: 0.0,
            size: 0.0,
        };
        let (text, _) = streaming.render_now(mtm, markdown, width);
        (streaming, text)
    }

    /// Updates only affected Markdown blocks; definitions and other global changes fall back to a full parse.
    pub(crate) fn render(
        &mut self,
        mtm: MainThreadMarker,
        markdown: &str,
        width: f64,
    ) -> Option<(Retained<NSAttributedString>, usize)> {
        let dark = super::super::figures::is_dark(mtm);
        let size = super::super::style::chat(super::super::style::BODY);
        if markdown == self.doc.text() && self.width == width && self.dark == dark && self.size == size {
            return None;
        }
        Some(self.render_now(mtm, markdown, width))
    }

    fn render_now(
        &mut self,
        mtm: MainThreadMarker,
        markdown: &str,
        width: f64,
    ) -> (Retained<NSAttributedString>, usize) {
        let dark = super::super::figures::is_dark(mtm);
        let changed = markdown != self.doc.text();
        let splice = if changed {
            match splat_markdown::incremental::reparse(&mut self.doc, markdown) {
                Some(splice) => Some(splice),
                None => {
                    self.doc = splat_markdown::reader::parse(markdown);
                    None
                }
            }
        } else {
            None
        };
        let asked = self.figures.refresh(&self.doc, splice.as_ref(), dark);
        super::super::figures::want_among(self.figures.keys(), asked);
        let macros = self.figures.macros().clone();
        let size = super::super::style::chat(super::super::style::BODY);
        let same_style = self.dark == dark && self.macros == macros && self.width == width && self.size == size;
        let ready = js::Ready {
            dark,
            macros: macros.clone(),
        };
        self.laid = match splice.filter(|_| same_style) {
            Some(splice) => relayout(
                &self.doc,
                &ready,
                None,
                std::mem::take(&mut self.laid),
                &reusable(&splice, &self.doc, None),
            ),
            None => layout(&self.doc, &ready, None),
        };
        if !same_style {
            self.blocks.clear();
        }
        self.dark = dark;
        self.macros = macros;
        self.width = width;
        self.size = size;
        self.render_blocks(mtm)
    }

    fn render_blocks(&mut self, mtm: MainThreadMarker) -> (Retained<NSAttributedString>, usize) {
        let renderer = Renderer {
            mtm,
            size: self.size,
            tone: Tone::Primary,
            dark: self.dark,
            macros: self.macros.clone(),
            width: self.width,
        };
        let old = std::mem::take(&mut self.blocks);
        let out = NSMutableAttributedString::new();
        let mut prefix = 0;
        let mut changed = false;
        let last_nonempty = self.laid.blocks.iter().rposition(|block| !block.paras.is_empty());
        for (i, block) in self.laid.blocks.iter().enumerate() {
            let last = Some(i) == last_nonempty;
            let cached = old
                .get(i)
                .filter(|cached| cached.key == block.key && cached.last == last);
            changed |= cached.is_none();
            let text = cached.map(|cached| cached.text.clone()).unwrap_or_else(|| {
                let paras: Vec<_> = block.paras.iter().map(|p| (0, p)).collect();
                render_paragraphs(&renderer, &paras, last)
            });
            if changed {
                out.appendAttributedString(&text);
            } else {
                prefix += text.length();
            }
            self.blocks.push(Block {
                key: block.key,
                last,
                text,
            });
        }
        (Retained::into_super(out), prefix)
    }
}
