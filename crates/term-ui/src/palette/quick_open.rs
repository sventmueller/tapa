//! The native search field and highlighted file paths of the quick open palette.

use std::ops::Range;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSColor, NSFont, NSLayoutAttribute, NSSearchField, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation,
    NSView,
};
use objc2_foundation::{NSEdgeInsets, NSMutableAttributedString, NSRange, NSStringCompareOptions};
use splat_i18n::t;
use term_render::constant;

use crate::kit::{label, ns, set_label};

pub(super) const WIDTH: f64 = 620.0;
pub(super) const HEIGHT: f64 = 380.0;
pub(super) const ROW_HEIGHT: f64 = 34.0;

pub(super) fn search_line(mtm: MainThreadMarker, placeholder: &str) -> (Retained<NSView>, Retained<NSTextField>) {
    let mode = label(mtm, t("Dateien"));
    mode.setFont(Some(&NSFont::systemFontOfSize_weight(
        11.0,
        constant!(objc2_app_kit::NSFontWeightSemibold),
    )));
    mode.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let search = NSSearchField::new(mtm);
    search.setPlaceholderString(Some(&ns(placeholder)));
    search.setFont(Some(&NSFont::systemFontOfSize(17.0)));
    search.setSendsSearchStringImmediately(true);
    set_label(&search, Some(placeholder));
    let header = NSStackView::new(mtm);
    header.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    header.setAlignment(NSLayoutAttribute::Leading);
    header.setSpacing(4.0);
    header.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 16.0,
        bottom: 8.0,
        right: 16.0,
    });
    header.addArrangedSubview(&mode);
    header.addArrangedSubview(&search);
    search
        .trailingAnchor()
        .constraintEqualToAnchor_constant(&header.trailingAnchor(), -16.0)
        .setActive(true);
    (header.into_super(), search.into_super())
}

pub(super) fn show_text(label: &NSTextField, path: &str, query: &str) {
    let string = ns(path);
    let text = NSMutableAttributedString::initWithString(NSMutableAttributedString::alloc(), &string);
    let accent = NSColor::controlAccentColor();
    let color = NSColor::labelColor();
    let font = NSFont::systemFontOfSize_weight(13.0, constant!(objc2_app_kit::NSFontWeightSemibold));
    let all = NSRange::new(0, string.length());
    // SAFETY: These AppKit attribute keys take a font and a colour respectively, over the full string.
    unsafe {
        text.addAttribute_value_range(constant!(objc2_app_kit::NSFontAttributeName), &font, all);
        text.addAttribute_value_range(constant!(objc2_app_kit::NSForegroundColorAttributeName), &color, all);
    }
    let query = query.trim();
    if !query.is_empty() {
        let found = string.rangeOfString_options(&ns(query), NSStringCompareOptions::CaseInsensitiveSearch);
        let ranges = if found.length > 0 {
            std::iter::once(found.location..found.location + found.length).collect()
        } else {
            fuzzy_ranges(path, query)
        };
        for range in ranges {
            // SAFETY: The ranges are UTF-16 offsets into `string`; the foreground attribute takes a colour.
            unsafe {
                text.addAttribute_value_range(
                    constant!(objc2_app_kit::NSForegroundColorAttributeName),
                    &accent,
                    NSRange::new(range.start, range.len()),
                );
            }
        }
    }
    label.setAttributedStringValue(&text);
}

/// Falls back to the matched characters of a subsequence when the query is not a contiguous part of the path.
fn fuzzy_ranges(path: &str, query: &str) -> Vec<Range<usize>> {
    let mut wanted = query.chars().filter(|ch| !ch.is_whitespace()).peekable();
    let mut matches: Vec<Range<usize>> = Vec::new();
    let mut offset = 0;
    for ch in path.chars() {
        let Some(next) = wanted.peek() else { break };
        let end = offset + ch.len_utf16();
        if ch.to_lowercase().eq(next.to_lowercase()) {
            wanted.next();
            if let Some(last) = matches.last_mut().filter(|last| last.end == offset) {
                last.end = end;
            } else {
                matches.push(offset..end);
            }
        }
        offset = end;
    }
    if wanted.peek().is_none() { matches } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_highlights_use_utf16_offsets_and_merge_adjacent_matches() {
        assert_eq!(fuzzy_ranges("🦀/tests/app.rs", "tas"), [3..4, 9..10, 14..15]);
        assert_eq!(fuzzy_ranges("src/test_file.rs", "tesf"), [4..7, 9..10]);
        assert!(fuzzy_ranges("src/main.rs", "test").is_empty());
    }
}
