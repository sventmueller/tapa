//! The formulas and diagrams of the answer that is still arriving, kept per top-level block, so that a chunk reads
//! only the blocks the partial parse replaced instead of the whole answer.

use std::sync::Arc;

use splat_markdown::doc::{
    Block, BlockKind, DiagramKind, Doc, Figure, InlineKind, Splice, diagram_kind, is_math_fence, macros_of,
    walk_blocks, walk_inlines,
};
use splat_render::js::Key;

/// Beyond this many, only the appearance shown is asked for; `splat_render::js::wanted` keeps the same bound private.
const RENDERED_AHEAD: usize = 200;

#[derive(Debug, Clone, PartialEq)]
enum Owned {
    Math { tex: String, display: bool },
    Diagram { source: String, kind: DiagramKind },
}

impl Owned {
    fn figure(&self) -> Figure<'_> {
        match self {
            Owned::Math { tex, display } => Figure::Math { tex, display: *display },
            Owned::Diagram { source, kind } => Figure::Diagram { source, kind: *kind },
        }
    }

    fn key(&self, dark: bool, macros: &Arc<str>) -> Key {
        match self {
            Owned::Math { tex, display } => Key::math(tex, *display, dark, macros),
            Owned::Diagram { source, kind } => Key::diagram(source, *kind, dark),
        }
    }

    fn defines(&self) -> bool {
        !macros_of(std::iter::once(&self.figure())).is_empty()
    }
}

/// The figures of `blocks` in source order, as `splat_markdown::doc::figures` finds them in a whole document.
fn figures_of(blocks: &[Block]) -> Vec<Owned> {
    let mut out = Vec::new();
    walk_blocks(blocks, &mut |b| {
        match &b.kind {
            BlockKind::MathBlock { literal } => out.push(Owned::Math {
                tex: literal.to_string(),
                display: true,
            }),
            BlockKind::CodeBlock { info, literal, .. } => {
                if let Some(kind) = diagram_kind(info) {
                    out.push(Owned::Diagram {
                        source: literal.to_string(),
                        kind,
                    });
                } else if is_math_fence(info) {
                    out.push(Owned::Math {
                        tex: literal.strip_suffix('\n').unwrap_or(literal).to_string(),
                        display: true,
                    });
                }
            }
            _ => {}
        }
        walk_inlines(b.inlines(), &mut |i| {
            if let InlineKind::Math { display, literal } = i.kind() {
                out.push(Owned::Math {
                    tex: literal.to_string(),
                    display: *display,
                });
            }
        });
    });
    out
}

/// What the answer needs rendered, as `splat_render::js::wanted` reads it, kept up to date block by block.
#[derive(Default)]
pub(super) struct Figures {
    blocks: Vec<Vec<Owned>>,
    macros: Arc<str>,
    keys: Vec<Key>,
    dark: bool,
}

impl Figures {
    /// The definitions the formulas share.
    pub(super) fn macros(&self) -> &Arc<str> {
        &self.macros
    }

    /// Everything the answer needs rendered, in the order `splat_render::js::wanted` gives it.
    pub(super) fn keys(&self) -> &[Key] {
        &self.keys
    }

    /// Catches up with `doc`, which `splice` changed or, without one, was parsed anew; the keys to ask for now.
    pub(super) fn refresh(&mut self, doc: &Doc, splice: Option<&Splice>, dark: bool) -> Vec<Key> {
        let replaced = splice.filter(|_| dark == self.dark).and_then(|s| self.splice(doc, s));
        let Some((old, new)) = replaced else {
            self.blocks = doc
                .blocks()
                .iter()
                .map(|b| figures_of(std::slice::from_ref(b)))
                .collect();
            self.dark = dark;
            self.macros = self.all_macros();
            self.keys = self.all_keys();
            return self.keys.clone();
        };
        if old == new {
            return Vec::new();
        }
        if old.iter().chain(&new).any(Owned::defines) {
            let macros = self.all_macros();
            if macros != self.macros {
                self.macros = macros;
                self.keys = self.all_keys();
                return self.keys.clone();
            }
        }
        self.keys = self.all_keys();
        let ahead = self.count() <= RENDERED_AHEAD;
        let fresh: Vec<&Owned> = new.iter().filter(|f| !old.contains(f)).collect();
        let mut ask: Vec<Key> = fresh.iter().map(|f| f.key(dark, &self.macros)).collect();
        if ahead {
            ask.extend(fresh.iter().map(|f| f.key(!dark, &self.macros)));
        }
        ask
    }

    /// Replaces the figures of the blocks `splice` replaced; the old ones and the new ones, `None` when the cache
    /// does not fit the splice.
    fn splice(&mut self, doc: &Doc, splice: &Splice) -> Option<(Vec<Owned>, Vec<Owned>)> {
        let blocks = doc.blocks();
        let fits = splice.old.end <= self.blocks.len()
            && splice.new.end <= blocks.len()
            && self.blocks.len() - splice.old.len() + splice.new.len() == blocks.len();
        if !fits {
            return None;
        }
        let added: Vec<Vec<Owned>> = blocks[splice.new.clone()]
            .iter()
            .map(|b| figures_of(std::slice::from_ref(b)))
            .collect();
        let new = added.iter().flatten().cloned().collect();
        let old = self.blocks.splice(splice.old.clone(), added).flatten().collect();
        Some((old, new))
    }

    fn count(&self) -> usize {
        self.blocks.iter().map(Vec::len).sum()
    }

    fn all_macros(&self) -> Arc<str> {
        let figures: Vec<Figure<'_>> = self.blocks.iter().flatten().map(Owned::figure).collect();
        macros_of(figures.iter()).into()
    }

    fn all_keys(&self) -> Vec<Key> {
        let all = || self.blocks.iter().flatten();
        let mut keys: Vec<Key> = all().map(|f| f.key(self.dark, &self.macros)).collect();
        if keys.len() <= RENDERED_AHEAD {
            keys.extend(all().map(|f| f.key(!self.dark, &self.macros)));
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = "Intro $a^2$ and $$\\newcommand{\\R}{\\mathbb{R}}$$\n\n```mermaid\ngraph TD\n```\n\n\
        > quoted $\\R$\n\n- item $x$\n\n```math\ny = 1\n```\n";

    #[test]
    fn the_figures_match_what_the_renderer_reads_from_the_whole_document() {
        for dark in [false, true] {
            let doc = splat_markdown::reader::parse(ANSWER);
            let wanted = splat_render::js::wanted(&doc, dark);
            let mut figures = Figures::default();
            let asked = figures.refresh(&doc, None, dark);
            assert_eq!(figures.macros(), &wanted.macros);
            assert_eq!(figures.keys(), wanted.keys.as_slice());
            assert_eq!(asked, wanted.keys);
        }
    }

    #[test]
    fn an_appended_chunk_asks_only_for_its_new_figures_and_keeps_the_whole_list() {
        let mut text = String::from("First $a$.\n\nSecond");
        let mut doc = splat_markdown::reader::parse(&text);
        let mut figures = Figures::default();
        figures.refresh(&doc, None, false);
        for (chunk, new) in [
            (" goes on", 0),
            (" with $b$", 2),
            (" and more", 0),
            ("\n\nThird $c$.", 2),
        ] {
            text.push_str(chunk);
            let splice = splat_markdown::incremental::reparse(&mut doc, &text).expect("a partial parse");
            let asked = figures.refresh(&doc, Some(&splice), false);
            assert_eq!(asked.len(), new, "after {chunk:?}");
            assert_eq!(figures.keys(), splat_render::js::wanted(&doc, false).keys.as_slice());
        }
    }

    #[test]
    fn a_new_definition_asks_for_everything_again() {
        let mut text = String::from("Uses $\\R$.\n\nNext");
        let mut doc = splat_markdown::reader::parse(&text);
        let mut figures = Figures::default();
        figures.refresh(&doc, None, false);
        text.push_str(" $\\def\\R{\\mathbb{R}}$");
        let splice = splat_markdown::incremental::reparse(&mut doc, &text).expect("a partial parse");
        let asked = figures.refresh(&doc, Some(&splice), false);
        let wanted = splat_render::js::wanted(&doc, false);
        assert_eq!(figures.macros(), &wanted.macros);
        assert_eq!(asked, wanted.keys);
    }
}
