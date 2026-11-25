//! Editor and preview scroll together: the line at the top of one is at the top of the other, matched through
//! `splat_preview::map`, by paragraph.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSScrollView, NSViewBoundsDidChangeNotification};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRange};
use splat_markdown::text::Byte;
use splat_preview::layout::Layout;
use splat_preview::map::{Index, ParaPos, Shown, preview_offset};

use objc2_app_kit::NSTextView;
use term_render::constant;
use term_ui::kit::{guarded, main_soon};
use term_ui_editor::EditorSerial;

use super::markdown::Preview;
use super::page::Views;

#[derive(Default)]
pub(crate) struct State {
    /// The source offset at the top of the preview, which a new text keeps in place when the editor is not on screen.
    top: Cell<Option<usize>>,
    /// Set while one side scrolls the other, so the other's answer does not scroll back.
    moving: Cell<bool>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// Which side scrolled since the other last followed: the editor, the preview.
    due: Cell<(bool, bool)>,
}

/// Lets the preview and its editor follow each other's scrolling.
pub(crate) fn attach(preview: &Rc<Preview>) {
    stop(preview);
    let key = preview.key;
    if let Some(scroll) = term_ui_editor::text_view(preview.editor()).and_then(|view| view.enclosingScrollView()) {
        observe(preview, &scroll, move || super::with(key, |p| scrolled(p, true)));
    }
    observe(preview, &preview.views.scroll, move || {
        super::with(key, |p| scrolled(p, false))
    });
}

/// One side scrolled: the other follows once the run loop comes round, once however often it scrolled meanwhile.
fn scrolled(preview: &Preview, editor: bool) {
    let state = &preview.sync;
    if state.moving.get() {
        return;
    }
    let (by_editor, by_preview) = state.due.get();
    state.due.set((by_editor || editor, by_preview || !editor));
    if by_editor || by_preview {
        return;
    }
    let key = preview.key;
    main_soon(move || super::with(key, |p| follow_due(p)));
}

/// Lets the other side follow what scrolled since the last time.
pub(super) fn follow_due(preview: &Preview) {
    let (by_editor, by_preview) = preview.sync.due.replace((false, false));
    if by_editor {
        editor_scrolled(preview);
    }
    if by_preview {
        preview_scrolled(preview);
    }
}

fn observe(preview: &Preview, scroll: &NSScrollView, scrolled: impl Fn() + 'static) {
    let clip = scroll.contentView();
    clip.setPostsBoundsChangedNotifications(true);
    let block = RcBlock::new(move |_: NonNull<NSNotification>| guarded((), &scrolled));
    let name = constant!(NSViewBoundsDidChangeNotification);
    // SAFETY: The block only looks up the preview by its number and does nothing once it is gone; the observer is
    // removed when the preview closes.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            Some(&clip),
            None,
            &block,
        )
    };
    preview.sync.observers.borrow_mut().push(observer);
}

/// Ends the following, as the preview closes or follows another editor.
pub(crate) fn stop(preview: &Preview) {
    let center = NSNotificationCenter::defaultCenter();
    for observer in preview.sync.observers.borrow_mut().drain(..) {
        // SAFETY: An observer this center handed out.
        unsafe { center.removeObserver(observer.as_ref()) };
    }
}

/// Runs `change` of the preview's text or place without the preview moving the editor.
pub(crate) fn while_moving(preview: &Preview, change: impl FnOnce()) {
    let was = preview.sync.moving.replace(true);
    change();
    preview.sync.moving.set(was);
}

/// Runs `change`, which replaces each range of the preview's text by text of the length beside it, keeping the line
/// at the top of the preview where it stands, as the editor did not move.
pub(crate) fn keeping_top(preview: &Preview, changes: &[(NSRange, usize)], change: impl FnOnce()) {
    let views = &preview.views;
    let visible = views.text.visibleRect().origin.y;
    let top = char_at_top(views)
        .filter(|&at| {
            visible > 0.0
                && !changes
                    .iter()
                    .any(|(range, _)| range.location <= at && at < range.end())
        })
        .and_then(|at| Some((at, y_of(views, at)? - visible)));
    while_moving(preview, change);
    let Some((at, offset)) = top else { return };
    let before: usize = changes
        .iter()
        .filter(|(range, _)| range.end() <= at)
        .map(|(range, _)| range.length)
        .sum();
    let after: usize = changes
        .iter()
        .filter(|(range, _)| range.end() <= at)
        .map(|(_, len)| len)
        .sum();
    let Some(y) = y_of(views, at + after - before) else {
        return;
    };
    while_moving(preview, || views.text.scrollPoint(NSPoint::new(0.0, y - offset)));
}

/// After a new text: the preview stands where the editor stands, or where it stood itself.
pub(crate) fn keep_place(preview: &Preview) {
    match visible_editor(preview) {
        Some(_) => editor_scrolled(preview),
        None => {
            if let Some(top) = preview.sync.top.get() {
                scroll_preview_to(preview, top);
            }
        }
    }
}

fn visible_editor(preview: &Preview) -> Option<Retained<NSTextView>> {
    term_ui_editor::text_view(preview.editor())
        .filter(|editor| editor.window().is_some() && !editor.isHiddenOrHasHiddenAncestor())
}

fn editor_scrolled(preview: &Preview) {
    if preview.sync.moving.get() {
        return;
    }
    let Some(editor) = visible_editor(preview) else { return };
    let Some(byte) = term_ui_editor::top_byte(preview.editor()) else {
        return;
    };
    while_moving(preview, || {
        let y = {
            let shown = preview.shown.borrow();
            let Some(anchors) = Anchors::new(preview.editor(), &editor, &preview.views, &shown, false) else {
                return;
            };
            let guess = anchors.index.paras[..anchors.paras]
                .partition_point(|place| para_src(anchors.laid, place).is_some_and(|src| src <= byte));
            anchors.follow(guess, editor.visibleRect().origin.y, |pair| pair)
        };
        preview.sync.top.set(Some(byte));
        if let Some(y) = y {
            preview.views.text.scrollPoint(NSPoint::new(0.0, y));
        }
    });
}

fn scroll_preview_to(preview: &Preview, byte: usize) {
    let y = {
        let shown = preview.shown.borrow();
        let Some(laid) = shown.layout.as_ref() else { return };
        if laid.blocks.is_empty() || !shown.index.fits(laid) {
            return;
        }
        let at = preview_offset(laid, &shown.index, Byte::new(byte));
        y_of(&preview.views, at.get())
    };
    let Some(y) = y else { return };
    preview.sync.top.set(Some(byte));
    while_moving(preview, || {
        let text = &preview.views.text;
        // The top inset stays above the first line.
        let y = if y <= super::page::INSET.height + 1.0 { 0.0 } else { y };
        text.scrollPoint(NSPoint::new(0.0, y));
    });
}

fn preview_scrolled(preview: &Preview) {
    if preview.sync.moving.get() {
        return;
    }
    let Some(byte) = top_of_preview(preview) else { return };
    preview.sync.top.set(Some(byte));
    let Some(editor) = visible_editor(preview) else { return };
    while_moving(preview, || {
        let y = {
            let shown = preview.shown.borrow();
            let Some(anchors) = Anchors::new(preview.editor(), &editor, &preview.views, &shown, true) else {
                return;
            };
            let guess = char_at_top(&preview.views)
                .and_then(|at| shown.index.para_at(Shown::new(at)))
                .map_or(0, |i| i + 1);
            anchors.follow(guess, preview.views.text.visibleRect().origin.y, |(e, p)| (p, e))
        };
        if let Some(y) = y {
            editor.scrollPoint(NSPoint::new(editor.visibleRect().origin.x, y));
        }
    });
}

/// Places that show the same text, as the editor's y and the preview's: anchor 0 is both tops, the last both ends, and
/// those between are the starts of the paragraphs in source order. Scrolling interpolates between the two around the
/// top, so that the following side moves as smoothly as the leading one.
struct Anchors<'a> {
    serial: EditorSerial,
    editor: &'a NSTextView,
    views: &'a Views,
    laid: &'a Layout,
    index: &'a Index,
    /// The paragraphs before the footnote definitions comrak moved to the end.
    paras: usize,
    /// Whether the editor follows and is scrolled to the answer, which then must not rest on estimated heights.
    editor_follows: bool,
}

impl<'a> Anchors<'a> {
    fn new(
        serial: EditorSerial,
        editor: &'a NSTextView,
        views: &'a Views,
        shown: &'a super::markdown::Shown,
        editor_follows: bool,
    ) -> Option<Self> {
        let laid = shown.layout.as_ref()?;
        let index = &shown.index;
        if laid.blocks.is_empty() || !index.fits(laid) {
            return None;
        }
        let paras = index.paras.partition_point(|place| place.block < index.in_order);
        Some(Self {
            serial,
            editor,
            views,
            laid,
            index,
            paras,
            editor_follows,
        })
    }

    fn last(&self) -> usize {
        self.paras + 1
    }

    fn get(&self, k: usize) -> Option<(f64, f64)> {
        if k == 0 {
            return Some((0.0, 0.0));
        }
        if k > self.paras {
            return Some((self.editor.frame().size.height, self.views.text.frame().size.height));
        }
        let place = self.index.paras.get(k - 1)?;
        let src = para_src(self.laid, place)?;
        let editor_y = term_ui_editor::byte_y(self.serial, src, !self.editor_follows)?;
        Some((editor_y, y_of(self.views, place.start.get())?))
    }

    /// The other side's y for `top` on the leading side, which `lead` puts first in each anchor; the search starts
    /// at anchor `guess`.
    fn follow(&self, guess: usize, top: f64, lead: fn((f64, f64)) -> (f64, f64)) -> Option<f64> {
        interpolate(self.last(), guess, top, |k| self.get(k).map(lead))
    }
}

/// The other side's y for `top` on the leading side, interpolated between anchors `0..=last` that `at` gives as
/// (leading y, following y); the search starts at anchor `guess`.
fn interpolate(last: usize, guess: usize, top: f64, at: impl Fn(usize) -> Option<(f64, f64)>) -> Option<f64> {
    let mut k = guess.min(last);
    let mut lower = at(k)?;
    while k > 0 && lower.0 > top {
        k -= 1;
        lower = at(k)?;
    }
    while k < last {
        let upper = at(k + 1)?;
        if upper.0 > top {
            let part = ((top - lower.0) / (upper.0 - lower.0)).clamp(0.0, 1.0);
            return Some(lower.1 + part * (upper.1 - lower.1));
        }
        k += 1;
        lower = upper;
    }
    Some(lower.1)
}

/// The source offset where paragraph `place` starts.
fn para_src(laid: &Layout, place: &ParaPos) -> Option<usize> {
    let block = laid.blocks.get(place.block)?;
    Some(
        block
            .paras
            .get(place.para)
            .map_or(block.src.start, |para| para.src.start)
            .get(),
    )
}

/// The source offset of the paragraph at the top of the preview.
fn top_of_preview(preview: &Preview) -> Option<usize> {
    let at = char_at_top(&preview.views)?;
    let shown = preview.shown.borrow();
    let laid = shown.layout.as_ref()?;
    if !shown.index.fits(laid) {
        return None;
    }
    let i = shown.index.para_at(Shown::new(at))?;
    para_src(laid, shown.index.paras.get(i)?)
}

/// The top of the line holding character `at` of the preview, in its text view's coordinates.
fn y_of(views: &Views, at: usize) -> Option<f64> {
    let length = views.storage.length();
    if length == 0 {
        return None;
    }
    let at = at.min(length - 1);
    // SAFETY: A range inside the text, and no pointer for the actual range, which the method allows.
    let glyphs = unsafe {
        views
            .layout
            .glyphRangeForCharacterRange_actualCharacterRange(NSRange::new(at, 1), std::ptr::null_mut())
    };
    let rect = views
        .layout
        .boundingRectForGlyphRange_inTextContainer(glyphs, &views.container);
    Some(rect.origin.y + views.text.textContainerOrigin().y)
}

/// The character at the top of what the preview shows.
pub(super) fn char_at_top(views: &Views) -> Option<usize> {
    if views.storage.length() == 0 {
        return None;
    }
    let visible = views.text.visibleRect();
    let origin = views.text.textContainerOrigin();
    let point = NSPoint::new(1.0, (visible.origin.y - origin.y).max(0.0) + 1.0);
    let glyph = views.layout.glyphIndexForPoint_inTextContainer(point, &views.container);
    Some(views.layout.characterIndexForGlyphAtIndex(glyph))
}

#[cfg(test)]
mod tests {
    use super::interpolate as follow;

    const ANCHORS: [(f64, f64); 4] = [(0.0, 0.0), (100.0, 300.0), (200.0, 400.0), (400.0, 1000.0)];

    fn at(k: usize) -> Option<(f64, f64)> {
        ANCHORS.get(k).copied()
    }

    #[test]
    fn a_top_between_anchors_lands_in_proportion_between_them() {
        assert_eq!(follow(3, 0, 50.0, at), Some(150.0));
        assert_eq!(follow(3, 0, 300.0, at), Some(700.0));
    }

    #[test]
    fn the_answer_does_not_depend_on_the_guess() {
        for guess in 0..=5 {
            assert_eq!(follow(3, guess, 150.0, at), Some(350.0), "guess {guess}");
        }
    }

    #[test]
    fn past_the_last_anchor_the_other_side_stays_at_its_end() {
        assert_eq!(follow(3, 1, 900.0, at), Some(1000.0));
    }

    #[test]
    fn a_missing_anchor_gives_no_place() {
        assert_eq!(follow(3, 0, 300.0, |k| if k == 2 { None } else { at(k) }), None);
    }
}
