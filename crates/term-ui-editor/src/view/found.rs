//! The match `reveal` marked in the find colour until the selection moves.

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::NSRange;
use term_document::Unit;

use super::EditorTextView;

impl EditorTextView {
    /// Marks `columns`, in UTF-16 code units, of line `line`, both counted from 0 and kept within the text, in the
    /// find colour and scrolls them into view; with `focus` it selects them and takes the keyboard focus, else the
    /// caret goes before them.
    pub(crate) fn select_columns(&self, line: usize, columns: std::ops::Range<usize>, focus: bool) {
        let range = {
            let document = self.document();
            let text = document.text();
            let line = u32::try_from(line.min(text.line_count().saturating_sub(1))).unwrap_or(u32::MAX);
            let at = |column: usize| {
                let column = u32::try_from(column).unwrap_or(u32::MAX);
                text.byte_to_utf16(text.lsp_offset(line, column, Unit::Utf16))
                    .unwrap_or(0)
            };
            let start = at(columns.start);
            NSRange::new(start, at(columns.end).saturating_sub(start))
        };
        self.setSelectedRange(if focus { range } else { NSRange::new(range.location, 0) });
        *self.ivars().found.borrow_mut() = Some(range.location..range.end());
        self.paint_found();
        self.scrollRangeToVisible(range);
        if focus && let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
    }

    /// Draws the revealed match in the find colour.
    pub(crate) fn paint_found(&self) {
        let Some(found) = self.ivars().found.borrow().clone() else {
            return;
        };
        let (Some(layout), Some(content)) = (self.textLayoutManager(), self.textContentStorage()) else {
            return;
        };
        let Some(range) = crate::text_view::text_range(&content, &found) else {
            return;
        };
        let marker: Retained<AnyObject> = objc2_app_kit::NSColor::findHighlightColor().into();
        let color: Retained<AnyObject> = objc2_app_kit::NSColor::blackColor().into();
        let (marker_key, color_key) = (
            crate::util::constant!(objc2_app_kit::NSBackgroundColorAttributeName),
            crate::util::constant!(objc2_app_kit::NSForegroundColorAttributeName),
        );
        // SAFETY: Both values are colours, as the background and foreground colour keys expect.
        unsafe {
            layout.addRenderingAttribute_value_forTextRange(marker_key, Some(&marker), &range);
            layout.addRenderingAttribute_value_forTextRange(color_key, Some(&color), &range);
        }
    }

    pub(crate) fn clear_found(&self) {
        let Some(found) = self.ivars().found.take() else { return };
        let (Some(layout), Some(content)) = (self.textLayoutManager(), self.textContentStorage()) else {
            return;
        };
        let Some(range) = crate::text_view::text_range(&content, &found) else {
            return;
        };
        layout.removeRenderingAttribute_forTextRange(
            crate::util::constant!(objc2_app_kit::NSBackgroundColorAttributeName),
            &range,
        );
        layout.removeRenderingAttribute_forTextRange(
            crate::util::constant!(objc2_app_kit::NSForegroundColorAttributeName),
            &range,
        );
        crate::highlight::repaint(self, &found);
    }
}
