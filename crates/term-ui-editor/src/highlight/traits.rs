//! Markdown's font traits, which change the layout and so live in the storage: painted only for the lines from the
//! top of the screen down, where TextKit lays the edited paragraphs out before it draws them, and only where the
//! storage shows other traits, so a restyle that leaves a line's traits as they were costs that line no layout.

use std::ops::Range;

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{NSFont, NSFontAttributeName, NSStrikethroughStyleAttributeName, NSTextStorage};
use objc2_foundation::{NSMutableAttributedString, NSNumber, NSObjectProtocol, NSRange};
use term_document::syntax::Styled;

use super::{line_range, part};
use crate::style::styled_code_font;
use crate::util::{constant, with_editing};
use crate::view::EditorTextView;

/// Bold, italic and struck through.
type Traits = (bool, bool, bool);

const PLAIN: Traits = (false, false, false);

/// Gives `lines` of `view`'s storage the font traits of the document's styles where they show others; whether the
/// view holds the document's text.
pub(super) fn paint(view: &EditorTextView, lines: Range<usize>) -> bool {
    let Some(storage) = view.storage() else { return false };
    let document = view.document();
    let text = document.text();
    let length = text.len_utf16();
    if storage.length() != length {
        return false;
    }
    let size = view.ivars().font_size.get();
    let stale: Vec<(NSRange, usize)> = lines
        .map_while(|line| Some((line_range(text, line, length)?, line)))
        .filter(|(whole, line)| !shown(&storage, *whole, document.styles(*line), size))
        .collect();
    if !stale.is_empty() {
        view.ivars().paint.styled.set(true);
        with_editing(&storage, || {
            for (whole, line) in stale {
                set(&storage, whole, document.styles(line), size);
            }
        });
        // Changed inside the viewport's layout, a paragraph keeps its new height but stays undrawn until the next one.
        view.later(sel!(termLayOutViewport), 0.0);
    }
    true
}

fn traits_of(styled: &Styled) -> Traits {
    (styled.style.bold, styled.style.italic, styled.style.strike)
}

/// The stretches of the line laid out at `line` with traits other than plain, and the plain ones between them; `None`
/// where styles overlap, which leaves the order they are set in to decide.
fn stretches(line: NSRange, styles: &[Styled]) -> Option<Vec<(NSRange, Traits)>> {
    let mut stretches = Vec::new();
    let mut at = line.location;
    for styled in styles.iter().filter(|s| traits_of(s) != PLAIN) {
        let Some(range) = part(line, styled.start, styled.end) else {
            continue;
        };
        if range.location < at {
            return None;
        }
        if range.location > at {
            stretches.push((NSRange::new(at, range.location - at), PLAIN));
        }
        stretches.push((range, traits_of(styled)));
        at = range.location + range.length;
    }
    let end = line.location + line.length;
    if end > at {
        stretches.push((NSRange::new(at, end - at), PLAIN));
    }
    Some(stretches)
}

/// Whether `storage` shows the traits of `styles` over the line laid out at `line`.
fn shown(storage: &NSTextStorage, line: NSRange, styles: &[Styled], size: f64) -> bool {
    stretches(line, styles).is_some_and(|stretches| {
        stretches.into_iter().all(|(range, (bold, italic, strike))| {
            let font = styled_code_font(size, bold, italic);
            let wanted: &AnyObject = &font;
            let font_shown = uniform(storage, constant!(NSFontAttributeName), range).is_some_and(|shown| {
                shown
                    .and_then(|f| f.downcast::<NSFont>().ok())
                    .is_some_and(|f| std::ptr::eq(&*f, &*font) || f.isEqual(Some(wanted)))
            });
            let strike_shown =
                uniform(storage, constant!(NSStrikethroughStyleAttributeName), range).is_some_and(|shown| {
                    let struck = shown
                        .and_then(|s| s.downcast::<NSNumber>().ok())
                        .is_some_and(|n| n.integerValue() != 0);
                    struck == strike
                });
            font_shown && strike_shown
        })
    })
}

/// The value of `key` over `range` of `storage` where it is the same throughout, as `Some(None)` where it is unset.
fn uniform(
    storage: &NSTextStorage,
    key: &objc2_foundation::NSString,
    range: NSRange,
) -> Option<Option<Retained<AnyObject>>> {
    let mut effective = NSRange::new(0, 0);
    // SAFETY: The range lies inside the storage, and `effective` is a valid place for the range found.
    let value =
        unsafe { storage.attribute_atIndex_longestEffectiveRange_inRange(key, range.location, &mut effective, range) };
    (effective.location == range.location && effective.length == range.length).then_some(value)
}

/// The plain code font and no strike-through over `range`.
pub(super) fn plain(string: &NSMutableAttributedString, range: NSRange, size: f64) {
    let (font, strike) = (
        constant!(NSFontAttributeName),
        constant!(NSStrikethroughStyleAttributeName),
    );
    // SAFETY: AppKit's font and strike-through keys, with a font as the value of the first.
    unsafe {
        string.addAttribute_value_range(font, &styled_code_font(size, false, false), range);
        string.removeAttribute_range(strike, range);
    }
}

/// Markdown's font traits of the line laid out at `line`.
pub(super) fn set(string: &NSMutableAttributedString, line: NSRange, styles: &[Styled], size: f64) {
    plain(string, line, size);
    let (font, strike) = (
        constant!(NSFontAttributeName),
        constant!(NSStrikethroughStyleAttributeName),
    );
    for styled in styles {
        let Some(range) = part(line, styled.start, styled.end) else {
            continue;
        };
        let style = styled.style;
        // SAFETY: AppKit's font and strike-through keys, with a font and a number as their values.
        unsafe {
            if style.bold || style.italic {
                string.addAttribute_value_range(font, &styled_code_font(size, style.bold, style.italic), range);
            }
            if style.strike {
                string.addAttribute_value_range(strike, &NSNumber::new_isize(1), range);
            }
        }
    }
}
