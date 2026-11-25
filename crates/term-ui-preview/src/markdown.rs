//! The preview of one Markdown document: laid out off the main thread by `splat_preview`, set as native text by
//! `splat_render`, with formulas and diagrams from the hidden renderer and pictures from files and the web.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::NSAppearanceCustomization;
use objc2_foundation::{NSMutableAttributedString, NSRange};
use splat_editor_state::state::links_of;
use splat_file_state::Stamp;
use splat_preview::layout::{Atom, Layout, RunMap, TopBlock, layout};
use splat_preview::map::Index;
use splat_render::Viewer;
use splat_render::images::{self, ImageFile, ImageStore};
use splat_render::js::{self, Key, Owner, Ready, Wanted};
use splat_render::picture::Rendered;
use splat_render::remote;
use splat_render::render::{Drawn, RenderCtx};

use term_base::{main_after, main_soon};
use term_ui::workspace_host::{Trust, Workspace, WorkspaceId};
use term_ui_editor::EditorSerial;

use super::page::Views;

/// The pause in typing after which the preview lays the text out again.
const LAYOUT_PAUSE: Duration = Duration::from_millis(120);

/// The pause in typing after which the formulas and diagrams the text holds are asked for, as splat waits.
const REQUEST_PAUSE: Duration = Duration::from_millis(350);

/// The pause after a layout before the disk is asked where its links lead, so typing a link does not ask per letter.
const LINK_PAUSE: Duration = Duration::from_millis(300);

/// What the preview shows now.
#[derive(Default)]
pub(crate) struct Shown {
    pub(crate) layout: Option<Layout>,
    pub(crate) index: Index,
    keys: Vec<Key>,
    macros: Arc<str>,
    dark: bool,
    /// The addresses of the pictures from the web the document names.
    web: Vec<String>,
}

/// What a block renders with besides itself: a block of the same key renders alike while this stays the same.
struct Look {
    dark: bool,
    macros: Arc<str>,
    width: f64,
    web: bool,
    zoom: f64,
}

impl PartialEq for Look {
    /// A width that changed less than the page lays pictures out anew for, as when a scroller shows, is the same.
    fn eq(&self, other: &Look) -> bool {
        self.dark == other.dark
            && self.macros == other.macros
            && (self.width - other.width).abs() < super::page::WIDTH_STEP
            && self.web == other.web
            && self.zoom.to_bits() == other.zoom.to_bits()
    }
}

pub(crate) struct Preview {
    pub(crate) key: u64,
    mtm: MainThreadMarker,
    workspace: WorkspaceId,
    editor: EditorSerial,
    path: PathBuf,
    pub(crate) views: Views,
    text: RefCell<term_document::Text>,
    images: RefCell<ImageStore>,
    viewer: OnceCell<Rc<dyn Viewer>>,
    owner: Cell<Option<Owner>>,
    pub(crate) shown: RefCell<Shown>,
    /// What the whole text was last rendered with, which the blocks set since were rendered with too.
    painted: RefCell<Option<Look>>,
    /// Counts the changes of the text; a layout of an older one does not replace that of a newer one.
    edits: Cell<u64>,
    laid_out: Cell<u64>,
    last_edit: Cell<Instant>,
    requested: RefCell<Option<Vec<Key>>>,
    render_due: Cell<bool>,
    /// The user asked for the pictures from the web of this document although its folder is not trusted.
    web_allowed: Cell<bool>,
    /// How much larger than splat's own size the text shows: as much as its editor's text is than the default.
    zoom: Cell<f64>,
    closed: Cell<bool>,
    pub(crate) sync: super::sync::State,
    links: super::links::LinkTargets,
}

impl Preview {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        workspace: WorkspaceId,
        editor: EditorSerial,
        path: &Path,
        key: u64,
        zoom: f64,
    ) -> Rc<Preview> {
        let name = super::display_name(path);
        let views = Views::new(mtm, key, &splat_i18n::tf("Vorschau: {}", &[&name]));
        Rc::new(Preview {
            key,
            mtm,
            workspace,
            editor,
            path: path.to_path_buf(),
            views,
            text: RefCell::new(term_ui_editor::text_snapshot(editor).unwrap_or_default()),
            images: RefCell::default(),
            viewer: OnceCell::new(),
            owner: Cell::new(None),
            shown: RefCell::default(),
            painted: RefCell::default(),
            edits: Cell::new(0),
            laid_out: Cell::new(0),
            last_edit: Cell::new(Instant::now()),
            requested: RefCell::new(None),
            render_due: Cell::new(false),
            web_allowed: Cell::new(false),
            zoom: Cell::new(zoom),
            closed: Cell::new(false),
            sync: super::sync::State::default(),
            links: super::links::LinkTargets::default(),
        })
    }

    /// The editor it follows.
    pub(crate) fn editor(&self) -> EditorSerial {
        self.editor
    }

    pub(crate) fn images(&self) -> &RefCell<ImageStore> {
        &self.images
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
            || !Workspace::new(self.workspace).is_open()
            || term_ui_editor::text_view(self.editor).is_none()
    }

    /// The preview as the render path knows it.
    pub(crate) fn viewer(self: &Rc<Self>) -> Rc<dyn Viewer> {
        self.viewer.get_or_init(|| Rc::new(Hears(Rc::downgrade(self)))).clone()
    }

    fn is_dark(&self) -> bool {
        let appearance = self.views.page.effectiveAppearance();
        matches!(
            splat_render::style::appearance_of(&appearance),
            splat_markdown::palette::Appearance::Dark | splat_markdown::palette::Appearance::DarkHighContrast
        )
    }

    /// Pictures from the web load when the folder is trusted or the user asked for them in this document (E22).
    pub(crate) fn loads_web_pictures(&self) -> bool {
        self.web_allowed.get() || Workspace::new(self.workspace).is_trusted()
    }

    pub(crate) fn allow_web_pictures(self: &Rc<Self>) {
        self.web_allowed.set(true);
        self.render();
    }

    /// The text changed: it is laid out again after a pause in typing.
    pub(crate) fn text_changed(&self) {
        let edits = self.edits.get() + 1;
        self.edits.set(edits);
        self.last_edit.set(Instant::now());
        let key = self.key;
        main_after(LAYOUT_PAUSE, move || {
            super::with(key, |preview| {
                if preview.edits.get() == edits {
                    preview.lay_out();
                }
            });
        });
    }

    /// Lays the text out on a background lane; what is ready of its formulas and diagrams shows as pictures.
    pub(crate) fn lay_out(&self) {
        if let Some(text) = term_ui_editor::text_snapshot(self.editor) {
            *self.text.borrow_mut() = text;
        }
        let snapshot = self.text.borrow().clone();
        let dark = self.is_dark();
        let (key, edits) = (self.key, self.edits.get());
        let doc_dir = self.path.parent().map(Path::to_path_buf);
        js::background(move || {
            let text = snapshot.to_string();
            let doc = splat_markdown::reader::parse(&text);
            let wanted = js::wanted(&doc, dark);
            let ready = Ready {
                dark,
                macros: wanted.macros.clone(),
            };
            let laid = layout(&doc, &ready, None);
            let links = super::links::wanted(&laid, &doc, &text, doc_dir.as_deref());
            main_soon(move || super::with(key, move |preview| preview.laid_out(edits, laid, wanted, dark, links)));
        });
    }

    fn laid_out(self: &Rc<Self>, edits: u64, laid: Layout, wanted: Wanted, dark: bool, links: super::links::Wanted) {
        if edits < self.laid_out.get() || self.closed.get() {
            return;
        }
        self.laid_out.set(edits);
        let old;
        {
            let mut shown = self.shown.borrow_mut();
            shown.index = Index::new(&laid);
            shown.web = web_pictures(&laid);
            old = shown.layout.replace(laid);
            shown.keys = wanted.keys;
            shown.macros = wanted.macros;
            shown.dark = dark;
        }
        self.links.take(links);
        let key = self.key;
        self.render_changes(old.as_ref());
        self.ask_when_quiet();
        main_after(LINK_PAUSE, move || {
            super::with(key, |preview| {
                if preview.edits.get() == edits {
                    preview.mark_links();
                    preview.ask_where_links_lead(false);
                }
            });
        });
    }

    /// Looks again where the links point, as files and notes come and go.
    pub(crate) fn look_at_links_again(self: &Rc<Self>) {
        self.ask_where_links_lead(true);
    }

    /// Asks the disk about the destinations of the links it has not answered for, and with `again` about the others.
    fn ask_where_links_lead(&self, again: bool) {
        if self.links.looking() {
            return;
        }
        let Some(wanted) = self.links.to_ask(again) else { return };
        let key = self.key;
        let asked = term_base::lane::DISK.ask(
            move || super::links::look(wanted),
            move |found| {
                let found = found.unwrap_or_default();
                main_soon(move || super::with(key, move |preview| preview.disk_answered(found)));
            },
        );
        if asked.is_err() {
            self.links.answered(Vec::new());
        }
    }

    fn disk_answered(self: &Rc<Self>, found: Vec<(splat_editor_state::links::Destination, Option<bool>)>) {
        self.links.answered(found);
        self.mark_links();
        self.ask_where_links_lead(false);
    }

    /// Sets again the blocks whose links gained or lost the mark of leading nowhere.
    fn mark_links(self: &Rc<Self>) {
        let changed = self.links.settle();
        if changed.is_empty() {
            return;
        }
        let parts = {
            let shown = self.shown.borrow();
            shown
                .layout
                .as_ref()
                .filter(|laid| self.shows(&shown, laid) && shown.index.fits(laid))
                .map(|laid| {
                    parts_where(laid, &shown.index, |block| {
                        links_of(block).any(|link| changed.contains(&link.attribute()))
                    })
                })
        };
        match parts {
            Some(parts) if parts.is_empty() => {}
            Some(parts) => self.paint(&parts),
            None => self.render(),
        }
    }

    /// Asks for the formulas and diagrams of the text once typing has paused.
    fn ask_when_quiet(self: &Rc<Self>) {
        let quiet = self.last_edit.get().elapsed();
        if quiet < REQUEST_PAUSE {
            let (key, edits) = (self.key, self.edits.get());
            main_after(REQUEST_PAUSE - quiet, move || {
                super::with(key, |preview| {
                    if preview.edits.get() == edits {
                        preview.ask_when_quiet();
                    }
                });
            });
            return;
        }
        let keys = self.shown.borrow().keys.clone();
        if self.requested.borrow().as_ref() == Some(&keys) {
            return;
        }
        *self.requested.borrow_mut() = Some(keys.clone());
        let viewer = self.viewer();
        if keys.is_empty() {
            js::asks_for_nothing(&viewer);
            return;
        }
        let renderer = js::renderer(self.mtm);
        let owner = self.owner.get().unwrap_or_else(|| renderer.listen(&viewer));
        self.owner.set(Some(owner));
        renderer.request(owner, keys);
    }

    /// Sets the blocks that hold pictures anew on a later turn of the run loop, once for any number of arrivals.
    pub(crate) fn render_soon(&self) {
        if self.render_due.replace(true) {
            return;
        }
        let key = self.key;
        main_soon(move || {
            super::with(key, |preview| {
                preview.render_due.set(false);
                preview.render_pictures();
            });
        });
    }

    /// Sets the whole layout as text, keeping the place the reader is at.
    pub(crate) fn render(self: &Rc<Self>) {
        let Some(count) = self.shown.borrow().layout.as_ref().map(|laid| laid.blocks.len()) else {
            return;
        };
        self.paint(&[(0..count, NSRange::new(0, self.views.storage.length()))]);
    }

    /// Sets only the blocks of the shown layout that differ from those of `old`, which the text shows; all of them
    /// when anything else they render with changed since.
    fn render_changes(self: &Rc<Self>, old: Option<&Layout>) {
        let part = {
            let shown = self.shown.borrow();
            old.filter(|old| self.shows(&shown, old))
                .zip(shown.layout.as_ref())
                .map(|(old, new)| changed(old, new))
        };
        match part {
            Some(part) => self.paint(&[part]),
            None => self.render(),
        }
    }

    /// Sets only the blocks of the shown layout that hold pictures, which the text shows; all of them when anything
    /// else they render with changed since.
    fn render_pictures(self: &Rc<Self>) {
        let parts = {
            let shown = self.shown.borrow();
            shown
                .layout
                .as_ref()
                .filter(|laid| self.shows(&shown, laid) && shown.index.fits(laid))
                .map(|laid| picture_parts(laid, &shown.index))
        };
        match parts {
            Some(parts) if parts.is_empty() => {}
            Some(parts) => self.paint(&parts),
            None => self.render(),
        }
    }

    /// Whether the text is `laid` as it would be rendered now.
    fn shows(&self, shown: &Shown, laid: &Layout) -> bool {
        self.painted.borrow().as_ref() == Some(&self.look(shown))
            && text_len(&laid.blocks) == self.views.storage.length()
    }

    /// What the blocks of the shown layout render with besides themselves.
    fn look(&self, shown: &Shown) -> Look {
        Look {
            dark: shown.dark,
            macros: shown.macros.clone(),
            width: self.views.picture_width(),
            web: self.loads_web_pictures(),
            zoom: self.zoom.get(),
        }
    }

    /// Shows the text `zoom` times as large as splat's own, rendering it anew when that changes its size.
    pub(crate) fn set_zoom(self: &Rc<Self>, zoom: f64) {
        if self.zoom.replace(zoom).to_bits() != zoom.to_bits() {
            self.render();
        }
    }

    /// Sets the blocks of the shown layout of each part in place of its range of the text, keeping the place the
    /// reader is at; the parts in order and apart.
    fn paint(self: &Rc<Self>, parts: &[Part]) {
        let viewer = self.viewer();
        let nowhere = self.links.nowhere();
        let left_out = Cell::new(false);
        let (look, set, local, names_web) = {
            let shown = self.shown.borrow();
            let Some(laid) = shown.layout.as_ref() else { return };
            let look = self.look(&shown);
            let drawn = Pictures {
                requester: &viewer,
                web: look.web,
                left_out: &left_out,
            };
            let ctx = RenderCtx {
                mtm: self.mtm,
                zoom: look.zoom,
                doc_dir: self.path.parent(),
                images: &self.images,
                max_image_width: look.width,
                scale: Some(
                    self.views
                        .text
                        .window()
                        .map_or(2.0, |window| window.backingScaleFactor()),
                ),
                dark: look.dark,
                requester: Some(&viewer),
                leaves_the_mac: None,
                leads_nowhere: Some(&nowhere),
                macros: look.macros.clone(),
                drawn: &drawn,
            };
            let set: Vec<_> = parts
                .iter()
                .map(|(blocks, replaced)| {
                    let out = NSMutableAttributedString::new();
                    for block in &laid.blocks[blocks.clone()] {
                        out.appendAttributedString(&ctx.render_block(block));
                    }
                    (*replaced, out)
                })
                .collect();
            (
                look,
                set,
                local_pictures(laid, self.path.parent()),
                !shown.web.is_empty(),
            )
        };
        let storage = &self.views.storage;
        let whole = matches!(parts, [(_, all)] if all.location == 0 && all.length == storage.length());
        let replace = || {
            storage.beginEditing();
            for (replaced, out) in set.iter().rev() {
                storage.replaceCharactersInRange_withAttributedString(*replaced, out);
            }
            storage.endEditing();
        };
        if whole {
            super::sync::while_moving(self, replace);
        } else {
            let changes: Vec<_> = set.iter().map(|(replaced, out)| (*replaced, out.length())).collect();
            super::sync::keeping_top(self, &changes, replace);
        }
        self.images.borrow_mut().keep_only(&local);
        if whole {
            *self.painted.borrow_mut() = Some(look);
            super::sync::keep_place(self);
        }
        // The blocks set again leave out the same pictures from the web as before.
        let offer = left_out.get() || (!whole && names_web && self.views.offers_web_pictures());
        self.views.offer_web_pictures(offer);
    }

    pub(crate) fn close(self: &Rc<Self>) {
        if self.closed.replace(true) {
            return;
        }
        super::sync::stop(self);
        splat_render::stops_showing(&self.viewer());
    }

    fn image_loaded(&self, path: PathBuf, file: ImageFile) {
        self.images.borrow_mut().finish(path, file);
        self.render_soon();
    }

    fn images_changed(self: &Rc<Self>, changed: Vec<(PathBuf, Stamp)>) {
        let viewer = self.viewer();
        for (path, now) in changed {
            self.images.borrow_mut().note_seen(&path, now);
            self.images.borrow().read_again(path, self.path.parent(), &viewer);
        }
    }
}

/// The length of the text `blocks` are set as.
fn text_len(blocks: &[Arc<TopBlock>]) -> usize {
    blocks.iter().map(|block| block.len_u16).sum()
}

/// A run of blocks of a layout and the part of the preview's text they replace.
type Part = (Range<usize>, NSRange);

/// The blocks of `new` that differ from those of `old`, and the part of the text set from `old` they replace: those
/// before the first change and after the last keep their text, though their source moved.
fn changed(old: &Layout, new: &Layout) -> Part {
    let same = |(a, b): &(&Arc<TopBlock>, &Arc<TopBlock>)| a.key == b.key && a.len_u16 == b.len_u16;
    let prefix = old.blocks.iter().zip(&new.blocks).take_while(same).count();
    let suffix = old.blocks[prefix..]
        .iter()
        .rev()
        .zip(new.blocks[prefix..].iter().rev())
        .take_while(same)
        .count();
    let start = text_len(&old.blocks[..prefix]);
    let len = text_len(&old.blocks[prefix..old.blocks.len() - suffix]);
    (prefix..new.blocks.len() - suffix, NSRange::new(start, len))
}

/// The runs of blocks of `laid` that hold pictures, each with the part of the text it is set as.
fn picture_parts(laid: &Layout, index: &Index) -> Vec<Part> {
    parts_where(laid, index, |block| {
        block.paras.iter().flat_map(|para| para.runs.iter()).any(|run| {
            matches!(
                &run.map,
                RunMap::Atom {
                    atom: Atom::Image(_),
                    ..
                }
            )
        })
    })
}

/// The runs of blocks of `laid` that `chosen` picks, each with the part of the text it is set as.
fn parts_where(laid: &Layout, index: &Index, chosen: impl Fn(&TopBlock) -> bool) -> Vec<Part> {
    let mut parts: Vec<Part> = Vec::new();
    for (at, block) in laid.blocks.iter().enumerate().filter(|(_, block)| chosen(block)) {
        match parts.last_mut() {
            Some((blocks, text)) if blocks.end == at => {
                blocks.end += 1;
                text.length += block.len_u16;
            }
            _ => parts.push((at..at + 1, NSRange::new(index.block_starts[at].get(), block.len_u16))),
        }
    }
    parts
}

/// The remote addresses of the pictures in `laid`.
fn web_pictures(laid: &Layout) -> Vec<String> {
    picture_urls(laid).filter(|url| remote::is_remote(url)).collect()
}

/// The files of the local pictures in `laid`, which the store keeps.
fn local_pictures(laid: &Layout, doc_dir: Option<&Path>) -> HashSet<PathBuf> {
    picture_urls(laid)
        .filter(|url| !remote::is_remote(url))
        .filter_map(|url| images::resolve_path(&url, doc_dir))
        .collect()
}

fn picture_urls(laid: &Layout) -> impl Iterator<Item = String> + '_ {
    laid.blocks
        .iter()
        .flat_map(|block| block.paras.iter())
        .flat_map(|para| para.runs.iter())
        .filter_map(|run| match &run.map {
            RunMap::Atom {
                atom: Atom::Image(image),
                ..
            } => Some(image.url.to_string()),
            _ => None,
        })
}

/// What a render gets from others: pictures from the web only where they may load, and what the renderer drew.
struct Pictures<'a> {
    requester: &'a Rc<dyn Viewer>,
    web: bool,
    /// Set when a picture from the web was left out.
    left_out: &'a Cell<bool>,
}

impl Drawn for Pictures<'_> {
    fn web_picture(&self, url: &str, need: splat_decoder::Fit) -> Option<Rendered> {
        if self.web {
            remote::get_for(url, Some(self.requester), need).flatten()
        } else {
            self.left_out.set(true);
            None
        }
    }

    fn drawing(&self, key: &Key) -> Option<Rendered> {
        js::cached(key)
    }
}

/// The preview as the render path reaches it, which does not keep it alive.
struct Hears(Weak<Preview>);

impl Viewer for Hears {
    fn renders_arrived(&self) {
        if let Some(preview) = self.0.upgrade() {
            let key = preview.key;
            main_soon(move || super::with(key, |preview| preview.lay_out()));
        }
    }

    fn image_arrived(&self, _url: String) {
        if let Some(preview) = self.0.upgrade() {
            preview.render_soon();
        }
    }

    fn image_loaded(&self, path: PathBuf, file: ImageFile) {
        if let Some(preview) = self.0.upgrade() {
            preview.image_loaded(path, file);
        }
    }

    fn images_changed(&self, changed: Vec<(PathBuf, Stamp)>) {
        if let Some(preview) = self.0.upgrade() {
            preview.images_changed(changed);
        }
    }

    fn web_images_shown(&self) -> Vec<String> {
        self.0
            .upgrade()
            .filter(|preview| preview.loads_web_pictures())
            .map(|preview| preview.shown.borrow().web.clone())
            .unwrap_or_default()
    }

    fn has_closed(&self) -> bool {
        self.0.upgrade().is_none_or(|preview| preview.is_closed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splat_preview::layout::NoImages;

    fn laid(text: &str) -> Layout {
        layout(&splat_markdown::reader::parse(text), &NoImages, None)
    }

    #[test]
    fn only_the_edited_block_is_set_again() {
        let old = laid("# Title\n\nSome text.\n\n- an item\n");
        let new = laid("# Title\n\nSome more text.\n\n- an item\n");
        let first = old.blocks[0].len_u16;
        assert_eq!(changed(&old, &new), (1..2, NSRange::new(first, old.blocks[1].len_u16)));
    }

    #[test]
    fn a_block_added_in_front_moves_none_of_those_behind_it() {
        let old = laid("Some text.\n\nMore text.\n");
        let new = laid("# New\n\nSome text.\n\nMore text.\n");
        assert_eq!(changed(&old, &new), (0..1, NSRange::new(0, 0)));
    }

    #[test]
    fn a_block_deleted_at_the_end_takes_its_text_along() {
        let old = laid("Some text.\n\nMore text.\n");
        let new = laid("Some text.\n");
        let first = old.blocks[0].len_u16;
        assert_eq!(changed(&old, &new), (1..1, NSRange::new(first, old.blocks[1].len_u16)));
        assert_eq!(text_len(&old.blocks), first + old.blocks[1].len_u16);
    }

    #[test]
    fn the_empty_text_gives_way_to_all_blocks() {
        let (old, new) = (laid(""), laid("Some text.\n\nMore text.\n"));
        assert_eq!(changed(&old, &new), (0..2, NSRange::new(0, text_len(&old.blocks))));
    }

    #[test]
    fn pictures_side_by_side_are_set_in_one_part() {
        let laid = laid("Text.\n\n![a](a.png)\n\n![b](b.png)\n\nMore.\n\n![c](c.png)\n");
        let index = Index::new(&laid);
        let at = |block: usize| index.block_starts[block].get();
        let len = |blocks: Range<usize>| text_len(&laid.blocks[blocks]);
        assert_eq!(
            picture_parts(&laid, &index),
            [
                (1..3, NSRange::new(at(1), len(1..3))),
                (4..5, NSRange::new(at(4), len(4..5)))
            ]
        );
    }
}
