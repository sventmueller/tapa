//! The visible screen as VoiceOver reads a text area: one string of lines, with offsets counted in UTF-16 code units as
//! AppKit's ranges are.

use objc2::DefinedClass;
use objc2_foundation::{NSRange, NSRect};
use term_session::{CellFlags, Session};

use super::view::TerminalView;

/// The visible lines joined by line breaks.
pub(crate) struct ScreenText {
    text: String,
    /// The offset at which each line starts.
    starts: Vec<usize>,
    length: usize,
}

impl ScreenText {
    pub(crate) fn new(lines: &[String]) -> ScreenText {
        let text = lines.join("\n");
        let mut starts = Vec::with_capacity(lines.len());
        let mut offset = 0;
        for line in lines {
            starts.push(offset);
            offset += line.encode_utf16().count() + 1;
        }
        let length = text.encode_utf16().count();
        ScreenText { text, starts, length }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn length(&self) -> usize {
        self.length
    }

    /// The line that holds `offset`; an offset past the end belongs to the last line.
    pub(crate) fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset).saturating_sub(1)
    }

    /// Where `line` starts and how long it is, its line break included.
    pub(crate) fn line_range(&self, line: usize) -> Option<(usize, usize)> {
        let start = *self.starts.get(line)?;
        let end = self.starts.get(line + 1).copied().unwrap_or(self.length);
        Some((start, end - start))
    }

    /// The offset of the `chars`-th character of `line`, at most its end.
    pub(crate) fn offset(&self, line: usize, chars: usize) -> usize {
        let Some(&start) = self.starts.get(line) else {
            return self.length;
        };
        let text = self.text.get(self.byte(start)..).unwrap_or("");
        let within: usize = text
            .chars()
            .take_while(|&c| c != '\n')
            .take(chars)
            .map(char::len_utf16)
            .sum();
        start + within
    }

    /// The text from `location`, `length` code units long, when both ends fall between characters.
    pub(crate) fn substring(&self, location: usize, length: usize) -> Option<String> {
        let end = location.checked_add(length).filter(|&end| end <= self.length)?;
        let units: Vec<u16> = self.text.encode_utf16().skip(location).take(end - location).collect();
        String::from_utf16(&units).ok()
    }

    /// The byte index of the UTF-16 `offset`.
    fn byte(&self, offset: usize) -> usize {
        let mut units = 0;
        for (index, c) in self.text.char_indices() {
            if units >= offset {
                return index;
            }
            units += c.len_utf16();
        }
        self.text.len()
    }
}

impl TerminalView {
    pub(super) fn screen_text(&self) -> ScreenText {
        ScreenText::new(&self.with_session(Session::visible_text).unwrap_or_default())
    }

    /// The cursor as an offset into `screen_text`.
    pub(super) fn cursor_offset(&self) -> usize {
        let text = self.screen_text();
        let screen = self.ivars().screen.borrow();
        let Some(screen) = screen.as_ref() else { return 0 };
        let point = screen.cursor.point;
        let chars = screen.rows.get(point.line).map_or(0, |row| {
            row.iter()
                .take(point.column)
                .filter(|cell| !cell.flags.contains(CellFlags::WIDE_SPACER))
                .map(|cell| 1 + cell.zero_width.len())
                .sum()
        });
        text.offset(point.line, chars)
    }

    pub(super) fn cursor_line(&self) -> isize {
        let line = self.ivars().screen.borrow().as_ref().map_or(0, |s| s.cursor.point.line);
        isize::try_from(line).unwrap_or(0)
    }

    /// The lines `range` touches, in screen coordinates.
    pub(super) fn frame_for_range(&self, range: NSRange) -> NSRect {
        let text = self.screen_text();
        let first = text.line_of(range.location);
        let last = text.line_of(range.location + range.length.saturating_sub(1)).max(first);
        let m = self.metrics();
        let width = self.bounds().size.width;
        let mut area = m.line_rect(first, width);
        area.size.height = m.cell_height * (last + 1 - first) as f64;
        let in_window = self.convertRect_toView(area, None);
        self.window()
            .map_or(in_window, |window| window.convertRectToScreen(in_window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_offsets_and_ranges_count_utf16_units() {
        let lines = vec!["$ echo 😀".to_string(), "😀".to_string(), String::new()];
        let screen = ScreenText::new(&lines);
        assert_eq!(screen.text(), "$ echo 😀\n😀\n");
        assert_eq!(screen.length(), 13);
        assert_eq!(screen.line_range(0), Some((0, 10)));
        assert_eq!(screen.line_range(1), Some((10, 3)));
        assert_eq!(screen.line_range(2), Some((13, 0)));
        assert_eq!(screen.line_range(3), None);
        assert_eq!(screen.line_of(0), 0);
        assert_eq!(screen.line_of(9), 0);
        assert_eq!(screen.line_of(10), 1);
        assert_eq!(screen.line_of(99), 2);
        assert_eq!(screen.offset(0, 2), 2);
        assert_eq!(screen.offset(1, 1), 12);
        assert_eq!(screen.offset(1, 9), 12, "past the end of the line");
        assert_eq!(screen.substring(7, 2).as_deref(), Some("😀"));
        assert_eq!(screen.substring(7, 1), None, "half a character");
        assert_eq!(screen.substring(12, 5), None);
    }
}
