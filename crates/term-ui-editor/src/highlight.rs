//! Colours of the text, lexed in slices that leave the main thread free. They are rendering attributes of the layout
//! manager, painted for the lines on screen and the rest once they come into view: an attribute of the storage would
//! make TextKit 2 lay its paragraph out again from estimated heights, which moves the lines on screen and the places a
//! jump lands on. Markdown's font traits change the layout and so go into the storage, but only for the lines on
//! screen (`traits`).

mod traits;
mod unpainted;

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSTextElementProvider;
use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::time::{Duration, Instant};

use objc2::sel;
use objc2_app_kit::{NSColor, NSForegroundColorAttributeName, NSTextContentStorage, NSTextLayoutManager, NSView};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize};
use term_document::syntax::Language;
use term_document::{Change, Text, Unit};

pub(crate) use self::unpainted::Unpainted;
use super::style::{palette_color, token_color};
use super::view::EditorTextView;
use crate::util::{constant, with_editing};

/// What an editor still has to paint.
#[derive(Default)]
pub(crate) struct Paint {
    /// The lines whose colours wait to be painted.
    pub(super) unpainted: RefCell<Unpainted>,
    /// The lines whose Markdown font traits wait to be painted into the storage.
    pub(super) unstyled: RefCell<Unpainted>,
    /// Markdown's font traits were painted into the storage, which another language takes out again.
    pub(super) styled: Cell<bool>,
}

/// How long one slice of background colouring may hold the main thread.
const SLICE: Duration = Duration::from_millis(4);

/// Lines beyond the screen coloured with it, so a short scroll shows colour at once.
const MARGIN: usize = 40;

/// Marks the lines `change` touched for painting, which `schedule` then does.
pub(crate) fn edited(view: &EditorTextView, change: &Change) {
    let (first, old_end, new_end) = (change.start.line, change.old_end.line, change.new_end.line);
    view.ivars()
        .paint
        .unpainted
        .borrow_mut()
        .edited(first, old_end, new_end);
    if is_markdown(view) {
        view.ivars().paint.unstyled.borrow_mut().edited(first, old_end, new_end);
    }
}

/// Marks `lines`, whose colours changed, for painting.
fn changed(view: &EditorTextView, lines: Range<usize>) {
    if is_markdown(view) {
        view.ivars().paint.unstyled.borrow_mut().add(lines.clone());
    }
    view.ivars().paint.unpainted.borrow_mut().add(lines);
}

/// Colours what an edit changed once the run loop comes round.
pub(crate) fn schedule(view: &EditorTextView) {
    view.later(sel!(termColorStep), 0.0);
}

fn is_markdown(view: &EditorTextView) -> bool {
    view.document().language().is_some_and(Language::is_markdown)
}

/// Lexes and paints the visible lines and, in a step that had nothing to paint on screen, lexes a slice of the rest
/// while the document is not lexed through. The step after a keystroke thus shares its turn of the run loop with
/// little more than the keystroke's own drawing, and catching up above the screen stops when the slice is over.
pub(crate) fn step(view: &EditorTextView) {
    let start = Instant::now();
    let visible = visible_lines(view);
    let lines = view.document_mut().highlight_within(visible.clone(), SLICE);
    if let Some(lines) = lines {
        changed(view, lines);
    }
    if !paint_due(view, &visible) {
        let more = view
            .document_mut()
            .highlight_more(SLICE.saturating_sub(start.elapsed()));
        if let Some(lines) = more {
            changed(view, lines);
            paint_due(view, &visible);
        }
    }
    if !view.document().is_highlighted() {
        view.later(sel!(termColorStep), 0.01);
    }
}

/// After TextKit laid out the lines on screen: those whose colours wait get them before they are drawn.
pub(crate) fn laid_out(view: &EditorTextView) {
    if view.ivars().paint.unpainted.borrow().is_empty() && view.ivars().paint.unstyled.borrow().is_empty() {
        return;
    }
    let visible = visible_lines(view);
    if view.ivars().paint.unpainted.borrow().meets(&visible)
        || view.ivars().paint.unstyled.borrow().meets(&on_screen(view, &visible))
    {
        paint_due(view, &visible);
    }
}

/// Colours everything again, after the whole text was replaced or its language changed.
pub(crate) fn recolor_all(view: &EditorTextView) {
    let markdown = is_markdown(view);
    if view.ivars().paint.styled.get() && !markdown {
        view.ivars().paint.styled.set(false);
        if let Some(storage) = view.storage() {
            let whole = NSRange::new(0, storage.length());
            with_editing(&storage, || {
                traits::plain(&storage, whole, view.ivars().font_size.get())
            });
        }
    }
    let lines = view.document().text().line_count();
    let visible = visible_lines(view);
    let _ = view.document_mut().highlight_within(visible.clone(), SLICE);
    *view.ivars().paint.unpainted.borrow_mut() = Unpainted::default();
    *view.ivars().paint.unstyled.borrow_mut() = Unpainted::default();
    changed(view, 0..lines.max(1));
    paint_due(view, &visible);
    schedule(view);
}

/// The lines of `visible` from the top of the screen down: the font traits of a line above it would make TextKit lay
/// it out from an estimated height, which moves the lines below.
fn on_screen(view: &EditorTextView, visible: &Range<usize>) -> Range<usize> {
    let top = line_at(view, NSPoint::new(0.0, view.visibleRect().origin.y)).unwrap_or(visible.start);
    top.clamp(visible.start, visible.end)..visible.end
}

/// Paints the lines of `lines` whose colours wait; whether there were any.
fn paint_due(view: &EditorTextView, lines: &Range<usize>) -> bool {
    let due = view.ivars().paint.unpainted.borrow_mut().take_in(lines);
    let restyle = view
        .ivars()
        .paint
        .unstyled
        .borrow_mut()
        .take_in(&on_screen(view, lines));
    if let Some(restyle) = restyle.clone() {
        if traits::paint(view, restyle.clone()) {
            // Markdown's font traits change the heights of the lines they style.
            super::viewport::forget(view);
        } else {
            view.ivars().paint.unstyled.borrow_mut().add(restyle);
        }
    }
    let Some(due) = due else { return restyle.is_some() };
    if let Some(utf16) = paint_shown(view, due.clone()) {
        if utf16.start < utf16.end {
            view.paint_found();
            redraw(view, &due);
        }
    } else {
        view.ivars().paint.unpainted.borrow_mut().add(due);
    }
    true
}

/// Has the paragraphs of `lines` that TextKit already drew draw again: a change of rendering attributes alone leaves
/// their drawing as it was, so colours painted after the first layout would not show until the line is laid out anew.
fn redraw(view: &EditorTextView, lines: &Range<usize>) {
    let rect = super::viewport::rect_of_lines(view, lines).unwrap_or_else(|| view.visibleRect());
    if rect.size.height > 0.0 {
        redraw_in(view, view, rect);
    }
}

/// Marks `rect` of `view`, in the coordinates of `editor`, and of every view inside it for drawing.
fn redraw_in(editor: &EditorTextView, view: &NSView, rect: NSRect) {
    for child in view.subviews().iter() {
        let local = child.convertRect_fromView(rect, Some(editor));
        let bounds = child.bounds();
        let top = local.origin.y.max(bounds.origin.y);
        let bottom = (local.origin.y + local.size.height).min(bounds.origin.y + bounds.size.height);
        if top < bottom {
            child.setNeedsDisplayInRect(NSRect::new(
                NSPoint::new(bounds.origin.x, top),
                NSSize::new(bounds.size.width, bottom - top),
            ));
            redraw_in(editor, &child, rect);
        }
    }
}

/// Gives the UTF-16 range `range` the colours of the lexing again, as after the find colour went.
pub(crate) fn repaint(view: &EditorTextView, range: &Range<usize>) {
    let lines = {
        let document = view.document();
        let text = document.text();
        let line = |utf16: usize| {
            text.utf16_to_byte(utf16.min(text.len_utf16()))
                .map_or(0, |byte| text.line_of(byte))
        };
        line(range.start)..line(range.end) + 1
    };
    let _ = view.ivars().paint.unpainted.borrow_mut().take_in(&lines);
    let _ = paint_shown(view, lines);
}

fn layout_of(view: &EditorTextView) -> Option<(Retained<NSTextLayoutManager>, Retained<NSTextContentStorage>)> {
    Some((view.textLayoutManager()?, view.textContentStorage()?))
}

/// Sets the colours of `lines` as rendering attributes from the document's spans; the UTF-16 range painted, unless
/// the view holds other text than the document.
fn paint_shown(view: &EditorTextView, lines: Range<usize>) -> Option<Range<usize>> {
    let (layout, content) = layout_of(view)?;
    let document = view.document();
    let text = document.text();
    let length = text.len_utf16();
    if view.storage()?.length() != length {
        return None;
    }
    let key = constant!(NSForegroundColorAttributeName);
    let plain: Retained<AnyObject> = NSColor::textColor().into();
    let mut painted: Option<Range<usize>> = None;
    for line in lines {
        let Some(whole) = line_range(text, line, length) else {
            break;
        };
        let utf16 = whole.location..whole.location + whole.length;
        let set = |range: &Range<usize>, color: &AnyObject| {
            if let Some(range) = crate::text_view::text_range(&content, range) {
                // SAFETY: The value of the foreground colour attribute is a colour.
                unsafe { layout.addRenderingAttribute_value_forTextRange(key, Some(color), &range) };
            }
        };
        set(&utf16, &plain);
        for span in document.spans(line) {
            if let Some(part) = part(whole, span.start, span.end) {
                let color: Retained<AnyObject> = token_color(span.kind).into();
                set(&(part.location..part.location + part.length), &color);
            }
        }
        for styled in document.styles(line) {
            if let (Some(token), Some(part)) = (styled.style.token, part(whole, styled.start, styled.end)) {
                let color: Retained<AnyObject> = palette_color(token).into();
                set(&(part.location..part.location + part.length), &color);
            }
        }
        painted = Some(painted.map_or(utf16.start, |p| p.start)..utf16.end);
    }
    painted
}

/// The lines on screen with a margin on both sides.
pub(super) fn visible_lines(view: &EditorTextView) -> Range<usize> {
    lines_in(view, view.visibleRect())
}

/// The lines laid out in `rect` of the view, with a margin on both sides.
pub(super) fn lines_in(view: &EditorTextView, rect: objc2_foundation::NSRect) -> Range<usize> {
    let top = line_at(view, NSPoint::new(0.0, rect.origin.y)).unwrap_or(0);
    let bottom = line_at(view, NSPoint::new(0.0, rect.origin.y + rect.size.height)).unwrap_or(top + 80);
    let count = view.document().text().line_count();
    top.saturating_sub(MARGIN)..(bottom + MARGIN).min(count)
}

/// The line laid out at `point` of the view.
pub(crate) fn line_at(view: &EditorTextView, point: NSPoint) -> Option<usize> {
    if let Some(line) = super::viewport::line_at_y(view, point.y) {
        return Some(line);
    }
    let layout = view.textLayoutManager()?;
    let content = view.textContentStorage()?;
    let origin = view.textContainerOrigin();
    let fragment = layout.textLayoutFragmentForPosition(NSPoint::new(point.x, point.y - origin.y))?;
    let start = content.documentRange().location();
    let offset = content.offsetFromLocation_toLocation(&start, &fragment.rangeInElement().location());
    let text = view.document();
    let byte = text.text().utf16_to_byte(usize::try_from(offset).ok()?).ok()?;
    Some(text.text().line_of(byte))
}

/// The UTF-16 range of line `line` with its line break.
fn line_range(text: &Text, line: usize, length: usize) -> Option<NSRange> {
    let utf16 = |byte: usize| text.byte_to_utf16(byte).ok();
    let start = text.offset(line, 0, Unit::Utf16).ok().and_then(utf16)?;
    let end = text
        .offset(line + 1, 0, Unit::Utf16)
        .ok()
        .and_then(utf16)
        .unwrap_or(length);
    Some(NSRange::new(start, end - start))
}

/// Columns `start..end` of a line laid out at `line`, where any of them are inside it.
fn part(line: NSRange, start: u32, end: u32) -> Option<NSRange> {
    let from = line.location + start as usize;
    let to = (line.location + end as usize).min(line.location + line.length);
    (from < to).then(|| NSRange::new(from, to - from))
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
