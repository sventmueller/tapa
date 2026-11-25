//! The text layout fragments on screen, enumerated once per layout pass and read by everything that draws beside or
//! over the text: the line numbers and the whitespace (SN-S0). It also holds the one observer of the editor's scrolling
//! that tells all of them.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::Bool;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSScrollView, NSTextElementProvider, NSTextLayoutFragment, NSTextLayoutFragmentEnumerationOptions,
    NSTextLayoutFragmentState, NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::view::EditorTextView;
use crate::util::constant;

/// Where a paragraph on screen lies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    /// Its first logical line, from 0.
    pub(crate) line: usize,
    /// Its frame in the view's coordinates.
    pub(crate) rect: NSRect,
    /// Whether it is the text's last paragraph.
    pub(crate) ends_text: bool,
}

impl Place {
    fn bottom(&self) -> f64 {
        self.rect.origin.y + self.rect.size.height
    }
}

/// What the layout looked like when the fragments were read; another key means they may lie elsewhere now.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    revision: term_document::Revision,
    generation: u64,
    width: u64,
    container: u64,
    font: u64,
}

/// The fragments of one layout pass, top to bottom.
pub(crate) struct Seen {
    key: Option<Key>,
    /// The heights of the view in which every fragment is in `places`, open at the text's ends.
    covered: Range<f64>,
    places: Vec<Place>,
    fragments: Vec<Retained<NSTextLayoutFragment>>,
}

impl Seen {
    /// The fragments that reach into the heights `top..=bottom` of the view, top to bottom.
    pub(crate) fn within(&self, top: f64, bottom: f64) -> impl Iterator<Item = (&Place, &NSTextLayoutFragment)> {
        let range = within(&self.places, top, bottom);
        self.places[range.clone()]
            .iter()
            .zip(self.fragments[range].iter().map(|f| &**f))
    }
}

/// The indices of the places of `places` that reach into the heights `top..=bottom`.
fn within(places: &[Place], top: f64, bottom: f64) -> Range<usize> {
    let first = places.partition_point(|p| p.bottom() <= top);
    let last = first + places[first..].partition_point(|p| p.rect.origin.y <= bottom);
    first..last
}

/// The line of the place of `places` at height `y`, or `None` when they do not say, `covered` being where they hold.
fn line_at(places: &[Place], covered: &Range<f64>, y: f64) -> Option<usize> {
    if !covered.contains(&y) {
        return None;
    }
    let place = places.get(places.partition_point(|p| p.rect.origin.y <= y).checked_sub(1)?)?;
    (y < place.bottom()).then_some(place.line)
}

/// The heights in which `places` are all there are, open above when they start the text and below when they end it.
fn covered(places: &[Place], starts_text: bool, ends_text: bool) -> Range<f64> {
    let top = match places.first() {
        Some(first) if !starts_text => first.rect.origin.y,
        _ => f64::NEG_INFINITY,
    };
    let bottom = match places.last() {
        Some(last) if !ends_text => last.bottom(),
        _ => f64::INFINITY,
    };
    top..bottom
}

fn covers(covered: &Range<f64>, rect: NSRect) -> bool {
    covered.start <= rect.origin.y && rect.origin.y + rect.size.height < covered.end
}

/// The fragments read last and what makes them stale, in the view's ivars.
#[derive(Default)]
pub(crate) struct Cache {
    seen: RefCell<Option<Rc<Seen>>>,
    /// Raised by whatever lays the text out again without an edit.
    generation: Cell<u64>,
}

fn key(view: &EditorTextView) -> Option<Key> {
    let revision = view.ivars().document.try_borrow().ok()?.revision();
    // SAFETY: A text view keeps its text container alive as long as it lives.
    let container = unsafe { view.textContainer() }.map_or(0.0, |c| c.size().width);
    Some(Key {
        revision,
        generation: view.ivars().viewport.generation.get(),
        width: view.frame().size.width.to_bits(),
        container: container.to_bits(),
        font: view.ivars().font_size.get().to_bits(),
    })
}

/// The fragments read last, if they still hold for the part of `view` on screen; TextKit also invalidates a fragment's
/// layout without an edit or a fold, as when the selection moves, and leaves it without line fragments until it lays
/// it out again.
fn fresh(view: &EditorTextView) -> Option<Rc<Seen>> {
    let seen = view.ivars().viewport.seen.borrow().clone()?;
    let visible = view.visibleRect();
    // Fragments read ahead of the screen may still wait for TextKit's layout; those on screen must not.
    let laid_out = || {
        seen.within(visible.origin.y, visible.origin.y + visible.size.height)
            .all(|(_, f)| f.state() == NSTextLayoutFragmentState::LayoutAvailable)
    };
    (seen.key.is_some() && seen.key == key(view) && covers(&seen.covered, visible) && laid_out()).then_some(seen)
}

/// The fragments on screen in `view`, read again only when the layout changed since.
pub(crate) fn seen(view: &EditorTextView) -> Rc<Seen> {
    if let Some(seen) = fresh(view) {
        return seen;
    }
    let visible = view.visibleRect();
    let heights = visible.origin.y..visible.origin.y + visible.size.height;
    let Some(seen) = read(view, heights, NSTextLayoutFragmentEnumerationOptions::EnsuresLayout) else {
        return Rc::new(empty());
    };
    let seen = Rc::new(seen);
    remember(view, seen.clone());
    seen
}

/// The fragments that reach into the heights `top..=bottom` of `view`: those on screen when they hold them, else
/// read and laid out for these heights alone.
pub(crate) fn seen_between(view: &EditorTextView, top: f64, bottom: f64) -> Rc<Seen> {
    let seen = seen(view);
    let heights = NSRect::new(NSPoint::new(0.0, top), NSSize::new(0.0, (bottom - top).max(0.0)));
    if seen.key.is_some() && covers(&seen.covered, heights) {
        return seen;
    }
    read(view, top..bottom, NSTextLayoutFragmentEnumerationOptions::EnsuresLayout)
        .map_or_else(|| Rc::new(empty()), Rc::new)
}

/// Keeps `seen` as the fragments read last; whether they lie otherwise than those before. The line numbers keep what
/// they drew while scrolling, so a line that moved redraws all of them.
fn remember(view: &EditorTextView, seen: Rc<Seen>) -> bool {
    let before = view.ivars().viewport.seen.replace(Some(seen.clone()));
    let Some(before) = before else {
        return true;
    };
    if before.places == seen.places {
        return false;
    }
    if moved(&before, &seen)
        && let Some(gutter) = view.ivars().around.gutter.borrow().as_ref().and_then(Weak::load)
    {
        gutter.redraw();
    }
    true
}

/// Whether a line read in both lies elsewhere in `now`, or either holds a line where the other held another.
fn moved(before: &Seen, now: &Seen) -> bool {
    shifted(&now.places, &before.places, &before.covered) || shifted(&before.places, &now.places, &now.covered)
}

/// Whether a place of `places` differs from that of its line in `others`, or lies where `others` held another line.
fn shifted(places: &[Place], others: &[Place], covered: &Range<f64>) -> bool {
    places.iter().any(
        |place| match others.binary_search_by_key(&place.line, |other| other.line) {
            Ok(index) => {
                let other = others[index].rect;
                other.origin.y.to_bits() != place.rect.origin.y.to_bits()
                    || other.size.height.to_bits() != place.rect.size.height.to_bits()
            }
            Err(_) => covered.contains(&place.rect.origin.y),
        },
    )
}

fn empty() -> Seen {
    Seen {
        key: None,
        covered: 0.0..0.0,
        places: Vec::new(),
        fragments: Vec::new(),
    }
}

/// The line laid out at height `y` of `view`, if the fragments read last know it.
pub(crate) fn line_at_y(view: &EditorTextView, y: f64) -> Option<usize> {
    let seen = fresh(view)?;
    line_at(&seen.places, &seen.covered, y)
}

/// Where the paragraphs of `lines` lie in `view` among the fragments read last, if those still hold: `Some` with an
/// empty rectangle when none of them is laid out among those.
pub(crate) fn rect_of_lines(view: &EditorTextView, lines: &Range<usize>) -> Option<NSRect> {
    let seen = fresh(view)?;
    Some(heights_of(&seen.places, lines).map_or(NSRect::ZERO, |heights| {
        NSRect::new(
            NSPoint::new(0.0, heights.start),
            NSSize::new(view.bounds().size.width, heights.end - heights.start),
        )
    }))
}

/// The heights the places of `places` for `lines` take up, if any.
fn heights_of(places: &[Place], lines: &Range<usize>) -> Option<Range<f64>> {
    let mut found = places.iter().filter(|place| lines.contains(&place.line));
    let first = found.next()?;
    let last = found.next_back().unwrap_or(first);
    Some(first.rect.origin.y..last.bottom())
}

/// The layout of `view` changed without an edit, as a fold changes it: the fragments read before are stale.
pub(crate) fn forget(view: &EditorTextView) {
    let generation = &view.ivars().viewport.generation;
    generation.set(generation.get().wrapping_add(1));
}

/// TextKit laid out the lines on screen: their fragments are read once for everyone, and what is drawn from them is
/// drawn again where they moved.
pub(crate) fn laid_out(view: &EditorTextView) {
    let _interval = term_base::signpost::begin(term_base::signpost::name!("viewport"));
    // The callback comes once the viewport is laid out; asking for layout from inside it would lay it out again.
    // Scrolling moves the screen, not the fragments: while it stays within what was read, nothing is read again.
    if fresh(view).is_some() {
        return;
    }
    let visible = view.visibleRect();
    let heights = visible.origin.y..visible.origin.y + visible.size.height;
    let ahead = visible.size.height * READ_AHEAD;
    let wide = (heights.start - ahead).max(0.0)..heights.end + ahead;
    let options = NSTextLayoutFragmentEnumerationOptions::empty();
    let before = view.ivars().viewport.seen.borrow().clone();
    let Some(seen) = before
        .filter(|before| before.key.is_some() && before.key == key(view))
        .and_then(|before| extend(view, &before, wide.clone()))
        .or_else(|| read(view, wide, options))
        .filter(|seen| covers(&seen.covered, visible))
        .or_else(|| read(view, heights, options))
    else {
        return;
    };
    if remember(view, Rc::new(seen)) {
        redraw_beside(view);
    }
}

/// Marks what draws from the fragments for drawing; AppKit draws each at most once per frame. The line numbers are
/// not among them: they scroll what they drew and draw only what comes into view.
fn redraw_beside(view: &EditorTextView) {
    super::whitespace::redraw(view);
}

/// Hands `visit` the fragments laid out from height `top` down, each found at the bottom of the one before, until it
/// stops or no fragment lies below; whether one lay at `top`. Enumerating fragments instead makes TextKit keep an element
/// for every paragraph up to the end, and each edit then shifts all of them.
fn step_down(
    layout: &objc2_app_kit::NSTextLayoutManager,
    origin_y: f64,
    top: f64,
    visit: &dyn Fn(&NSTextLayoutFragment) -> bool,
) -> bool {
    let mut y = top - origin_y;
    let mut first = true;
    while let Some(fragment) = layout.textLayoutFragmentForPosition(NSPoint::new(0.0, y)) {
        first = false;
        if !visit(&fragment) {
            return true;
        }
        let frame = fragment.layoutFragmentFrame();
        let below = frame.origin.y + frame.size.height;
        if below <= y {
            return true;
        }
        y = below;
    }
    !first
}

/// The fragments of `before` that still reach into the heights `wide`, with those beyond them read anew: a scroll
/// brings in a strip at one edge, so only that strip is looked up. `None` when nothing of `before` lies in `wide`.
fn extend(view: &EditorTextView, before: &Seen, wide: Range<f64>) -> Option<Seen> {
    let keep = within(&before.places, wide.start, wide.end);
    let (first, last) = (
        before.places.get(keep.start)?,
        before.places.get(keep.end.checked_sub(1)?)?,
    );
    let options = NSTextLayoutFragmentEnumerationOptions::empty();
    let mut places = Vec::new();
    let mut fragments = Vec::new();
    let mut top = if keep.start > 0 {
        first.rect.origin.y
    } else {
        before.covered.start
    };
    if top > wide.start {
        let above = read(view, wide.start..first.rect.origin.y, options)?;
        for (place, fragment) in above.places.iter().zip(&above.fragments) {
            if place.line < first.line {
                places.push(*place);
                fragments.push(fragment.clone());
            }
        }
        top = above.covered.start;
    }
    places.extend_from_slice(&before.places[keep.clone()]);
    fragments.extend(before.fragments[keep.clone()].iter().cloned());
    let mut bottom = if keep.end < before.places.len() {
        last.bottom()
    } else {
        before.covered.end
    };
    if bottom < wide.end {
        let below = read(view, last.bottom()..wide.end, options)?;
        for (place, fragment) in below.places.iter().zip(&below.fragments) {
            if place.line > last.line {
                places.push(*place);
                fragments.push(fragment.clone());
            }
        }
        bottom = below.covered.end;
    }
    Some(Seen {
        key: before.key,
        covered: top..bottom,
        places,
        fragments,
    })
}

/// How much of a screen above and below the screen the fragments are read, so a smooth scroll reads them only now and
/// then.
const READ_AHEAD: f64 = 0.5;

/// What one enumeration found.
#[derive(Default)]
struct Found {
    places: Vec<Place>,
    fragments: Vec<Retained<NSTextLayoutFragment>>,
    /// Whether the enumeration stopped at the edge of the screen rather than at the text's end.
    stopped: bool,
}

/// Reads the fragments that reach into the heights `heights` of `view`, with `options`; `None` while the document is
/// being changed.
fn read(view: &EditorTextView, heights: Range<f64>, options: NSTextLayoutFragmentEnumerationOptions) -> Option<Seen> {
    let key = key(view)?;
    let (layout, content) = (view.textLayoutManager()?, view.textContentStorage()?);
    let origin = view.textContainerOrigin();
    let (top, bottom) = (heights.start, heights.end);
    // Where TextKit has laid out nothing yet, as after a jump, the viewport it lays out next is the place to start:
    // walking there from either end would lay out and keep every paragraph on the way, which each edit then shifts.
    let start = layout
        .textLayoutFragmentForPosition(NSPoint::new(0.0, top - origin.y))
        .map(|fragment| fragment.rangeInElement().location())
        .or_else(|| {
            layout
                .textViewportLayoutController()
                .viewportRange()
                .map(|range| range.location())
        });
    // Past the text's end no fragment answers; walking back from the end finds the last ones without laying out all.
    let backwards = start.is_none() && top - origin.y > 0.0;
    let whole = content.documentRange();
    let document = view.ivars().document.try_borrow().ok()?;
    let text = document.text();
    let found = RefCell::new(Found::default());
    let visit = |fragment: &NSTextLayoutFragment| -> bool {
        let frame = fragment.layoutFragmentFrame();
        let rect = NSRect::new(
            NSPoint::new(frame.origin.x + origin.x, frame.origin.y + origin.y),
            NSSize::new(frame.size.width, frame.size.height),
        );
        let past = if backwards {
            rect.origin.y + rect.size.height <= top
        } else {
            rect.origin.y > bottom
        };
        if past {
            found.borrow_mut().stopped = true;
            return false;
        }
        let range = fragment.rangeInElement();
        let offset = content.offsetFromLocation_toLocation(&whole.location(), &range.location());
        let Some(line) = usize::try_from(offset)
            .ok()
            .and_then(|o| text.utf16_to_byte(o).ok())
            .map(|byte| text.line_of(byte))
        else {
            return true;
        };
        let ends_text = content.offsetFromLocation_toLocation(&range.endLocation(), &whole.endLocation()) == 0;
        let mut found = found.borrow_mut();
        found.places.push(Place { line, rect, ends_text });
        found.fragments.push(objc2::Message::retain(fragment));
        true
    };
    if options.is_empty() && !backwards && step_down(&layout, origin.y, top, &visit) {
        // Each fragment was found by its place, as the one below the last.
    } else {
        let block = RcBlock::new(|fragment: NonNull<NSTextLayoutFragment>| -> Bool {
            // SAFETY: TextKit keeps the fragment alive for the call.
            crate::util::guarded(Bool::NO, || Bool::new(visit(unsafe { fragment.as_ref() })))
        });
        let options = if backwards {
            options | NSTextLayoutFragmentEnumerationOptions::Reverse
        } else {
            options
        };
        let _ = layout.enumerateTextLayoutFragmentsFromLocation_options_usingBlock(start.as_deref(), options, &block);
    }
    drop(document);
    let mut found = found.into_inner();
    if backwards {
        found.places.reverse();
        found.fragments.reverse();
    }
    let first_line = found.places.first().is_some_and(|p| p.line == 0);
    let (starts_text, ends_text) = if backwards {
        (!found.stopped || first_line, true)
    } else {
        (start.is_none() || first_line, !found.stopped)
    };
    Some(Seen {
        key: Some(key),
        covered: covered(&found.places, starts_text, ends_text),
        places: found.places,
        fragments: found.fragments,
    })
}

/// Tells everything that follows `view`'s scrolling, with one observer for all of them, as `scroll` scrolls.
pub(crate) fn follow(scroll: &NSScrollView, view: &EditorTextView) {
    let clip = scroll.contentView();
    clip.setPostsBoundsChangedNotifications(true);
    let editor = Weak::from(view);
    let name = constant!(NSViewBoundsDidChangeNotification);
    let watch = crate::util::observe(view.mtm(), name, Some(&clip), move |_| {
        if let Some(editor) = editor.load() {
            scrolled(&editor);
        }
    });
    view.ivars().watches.borrow_mut().push(watch);
}

fn scrolled(view: &EditorTextView) {
    let _interval = term_base::signpost::begin(term_base::signpost::name!("scroll"));
    if let Some(gutter) = view.ivars().around.gutter.borrow().as_ref().and_then(Weak::load) {
        gutter.follow(view);
    }
    redraw_beside(view);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(rows: &[(usize, f64, f64)]) -> Vec<Place> {
        rows.iter()
            .map(|&(line, y, height)| Place {
                line,
                rect: NSRect::new(NSPoint::new(0.0, y), NSSize::new(100.0, height)),
                ends_text: false,
            })
            .collect()
    }

    #[test]
    fn the_fragments_within_heights_are_those_that_reach_into_them() {
        let rows = places(&[(3, 100.0, 20.0), (4, 120.0, 20.0), (7, 140.0, 40.0), (8, 180.0, 20.0)]);
        assert_eq!(within(&rows, 125.0, 150.0), 1..3);
        assert_eq!(
            within(&rows, 120.0, 140.0),
            1..3,
            "a fragment ending at the top is out, one starting at the bottom in"
        );
        assert_eq!(within(&rows, 0.0, 99.0), 0..0);
        assert_eq!(within(&rows, 0.0, 1000.0), 0..4);
    }

    #[test]
    fn painted_lines_take_up_the_heights_of_their_fragments_on_screen() {
        let rows = places(&[(3, 100.0, 20.0), (4, 120.0, 20.0), (7, 140.0, 40.0), (8, 180.0, 20.0)]);
        assert_eq!(heights_of(&rows, &(4..8)), Some(120.0..180.0));
        assert_eq!(heights_of(&rows, &(0..4)), Some(100.0..120.0));
        assert_eq!(
            heights_of(&rows, &(9..20)),
            None,
            "lines not laid out are drawn when they are"
        );
    }

    #[test]
    fn a_height_has_the_line_of_its_fragment_and_none_outside_what_was_read() {
        let rows = places(&[(3, 100.0, 20.0), (4, 120.0, 20.0), (7, 140.0, 40.0)]);
        let covered = covered(&rows, false, false);
        assert_eq!(covered, 100.0..180.0);
        assert_eq!(line_at(&rows, &covered, 100.0), Some(3));
        assert_eq!(line_at(&rows, &covered, 139.9), Some(4));
        assert_eq!(
            line_at(&rows, &covered, 170.0),
            Some(7),
            "a folded fragment answers with its first line"
        );
        assert_eq!(line_at(&rows, &covered, 99.0), None);
        assert_eq!(line_at(&rows, &covered, 180.0), None);
    }

    fn seen(rows: &[(usize, f64, f64)], starts_text: bool, ends_text: bool) -> Seen {
        let places = places(rows);
        Seen {
            key: None,
            covered: covered(&places, starts_text, ends_text),
            places,
            fragments: Vec::new(),
        }
    }

    #[test]
    fn scrolling_moves_no_line_but_a_taller_or_folded_one_does() {
        let before = seen(&[(3, 100.0, 20.0), (4, 120.0, 20.0), (5, 140.0, 20.0)], false, false);
        let scrolled = seen(&[(5, 140.0, 20.0), (6, 160.0, 20.0), (7, 180.0, 20.0)], false, false);
        assert!(
            !moved(&before, &scrolled),
            "lines read anew below, the shared one in its place"
        );
        assert!(!moved(&scrolled, &before), "lines read anew above");
        let taller = seen(&[(3, 100.0, 20.0), (4, 120.0, 34.0), (5, 154.0, 20.0)], false, false);
        assert!(moved(&before, &taller), "an inset made a line taller");
        let folded = seen(&[(3, 100.0, 20.0), (4, 120.0, 20.0), (9, 140.0, 20.0)], false, false);
        assert!(moved(&before, &folded), "another line where line 5 was");
    }

    #[test]
    fn what_was_read_covers_the_screen_only_within_its_fragments_or_up_to_the_texts_ends() {
        let rows = places(&[(0, 8.0, 20.0), (1, 28.0, 20.0)]);
        let screen = |y: f64, height: f64| NSRect::new(NSPoint::new(0.0, y), NSSize::new(100.0, height));
        let open = covered(&rows, true, true);
        assert!(covers(&open, screen(0.0, 500.0)), "a short text covers any screen");
        let closed = covered(&rows, false, false);
        assert!(covers(&closed, screen(10.0, 30.0)));
        assert!(!covers(&closed, screen(0.0, 30.0)));
        assert!(!covers(&closed, screen(10.0, 40.0)));
        assert_eq!(covered(&[], false, false), f64::NEG_INFINITY..f64::INFINITY);
    }
}
