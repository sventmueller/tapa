//! A text in lines, and its places as bytes, UTF-16 code units and lines with columns in every unit.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use ropey::{Rope, RopeSlice};

/// The unit a column or an offset counts in. A language server negotiates one of these; `Utf32` counts characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Bytes of UTF-8.
    Utf8,
    /// Code units of UTF-16, as AppKit and most language servers count.
    Utf16,
    /// Unicode scalar values.
    Utf32,
}

/// A place in a text: its line and its column in each unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Point {
    /// The line, from 0.
    pub line: usize,
    /// The column in bytes of UTF-8.
    pub utf8: usize,
    /// The column in code units of UTF-16.
    pub utf16: usize,
    /// The column in characters.
    pub utf32: usize,
}

impl Point {
    /// The column in `unit`.
    pub fn column(&self, unit: Unit) -> usize {
        match unit {
            Unit::Utf8 => self.utf8,
            Unit::Utf16 => self.utf16,
            Unit::Utf32 => self.utf32,
        }
    }
}

/// Why an offset or a position names no place in a text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionError {
    /// Beyond the end of the text or of the line.
    OutOfBounds,
    /// Between the bytes or the two UTF-16 halves of one character.
    InsideCharacter,
}

impl fmt::Display for PositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            PositionError::OutOfBounds => "the position is beyond the text",
            PositionError::InsideCharacter => "the position is inside a character",
        })
    }
}

impl std::error::Error for PositionError {}

/// A text in lines. A clone shares the text and takes constant time, so a copy can go to another thread while the
/// original changes. Lines end at LF, CRLF and a lone CR, as for a language server; every lookup takes O(log n).
#[derive(Clone, Default, PartialEq)]
pub struct Text {
    rope: Rope,
}

impl fmt::Debug for Text {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Text")
            .field("bytes", &self.len_bytes())
            .field("lines", &self.line_count())
            .finish()
    }
}

impl fmt::Display for Text {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.rope.chunks().try_for_each(|chunk| formatter.write_str(chunk))
    }
}

impl From<&str> for Text {
    fn from(text: &str) -> Text {
        Text::new(text)
    }
}

impl Text {
    /// The text `text`.
    pub fn new(text: &str) -> Text {
        Text {
            rope: Rope::from_str(text),
        }
    }

    /// The rope underneath, for a copy that shares its leaves, such as Vim's.
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub(crate) fn rope_mut(&mut self) -> &mut Rope {
        &mut self.rope
    }

    /// The length in bytes of UTF-8.
    pub(crate) fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// The length in code units of UTF-16.
    pub fn len_utf16(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// How many lines the text has: one more than line breaks, so a text that ends in a break has an empty last line.
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// The pieces the text is stored in, in order.
    pub(crate) fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// The bytes `range` of the text, if it lies on character boundaries within it.
    #[cfg(test)]
    pub(crate) fn slice(&self, range: Range<usize>) -> Option<Cow<'_, str>> {
        self.rope_slice(range).map(Cow::from)
    }

    /// Whether the bytes `range` of the text are those of `other`, if the range lies on character boundaries within
    /// both; compared piece by piece, without a copy of either.
    pub(crate) fn same_bytes(&self, other: &Text, range: Range<usize>) -> Option<bool> {
        let ours = self.rope_slice(range.clone())?;
        let theirs = other.rope_slice(range)?;
        Some(ours == theirs)
    }

    /// Whether the bytes `range` of the text are `piece`; never when the range does not lie on character boundaries
    /// within it.
    pub fn has_at(&self, range: Range<usize>, piece: &str) -> bool {
        self.rope_slice(range).is_some_and(|slice| slice == piece)
    }

    fn rope_slice(&self, range: Range<usize>) -> Option<RopeSlice<'_>> {
        if range.start > range.end || !self.is_boundary(range.start) || !self.is_boundary(range.end) {
            return None;
        }
        Some(self.rope.byte_slice(range))
    }

    /// Line `line` with its line break, if the text has it.
    pub fn line(&self, line: usize) -> Option<Cow<'_, str>> {
        self.rope.get_line(line).map(Cow::from)
    }

    /// The first `chars` characters of line `line`, its line break among them when the line is that short, if the
    /// text has the line.
    pub fn line_head(&self, line: usize, chars: usize) -> Option<Cow<'_, str>> {
        let whole = self.rope.get_line(line)?;
        Some(whole.slice(..chars.min(whole.len_chars())).into())
    }

    /// The bytes of line `line` with its line break.
    pub(crate) fn line_bytes(&self, line: usize) -> Option<Range<usize>> {
        (line < self.line_count()).then(|| self.rope.line_to_byte(line)..self.rope.line_to_byte(line + 1))
    }

    /// The bytes of line `line` without its line break.
    pub fn line_content(&self, line: usize) -> Option<Range<usize>> {
        (line < self.line_count()).then(|| self.rope.line_to_byte(line)..self.rope.char_to_byte(self.content_end(line)))
    }

    /// The line the byte `byte` is on; the last line for offsets beyond the end.
    pub fn line_of(&self, byte: usize) -> usize {
        self.rope.byte_to_line(byte.min(self.len_bytes()))
    }

    /// The character after the last one of line `line` that is not part of its line break.
    fn content_end(&self, line: usize) -> usize {
        let start = self.rope.line_to_char(line);
        let mut end = self.rope.line_to_char(line + 1);
        if end > start && self.rope.char(end - 1) == '\n' {
            end -= 1;
        }
        if end > start && self.rope.char(end - 1) == '\r' {
            end -= 1;
        }
        end
    }

    fn is_boundary(&self, byte: usize) -> bool {
        self.char_at_byte(byte).is_ok()
    }

    fn char_at_byte(&self, byte: usize) -> Result<usize, PositionError> {
        let char = self
            .rope
            .try_byte_to_char(byte)
            .map_err(|_| PositionError::OutOfBounds)?;
        if self.rope.char_to_byte(char) == byte {
            Ok(char)
        } else {
            Err(PositionError::InsideCharacter)
        }
    }

    fn char_at_utf16(&self, utf16: usize) -> Result<usize, PositionError> {
        let char = self
            .rope
            .try_utf16_cu_to_char(utf16)
            .map_err(|_| PositionError::OutOfBounds)?;
        if self.rope.char_to_utf16_cu(char) == utf16 {
            Ok(char)
        } else {
            Err(PositionError::InsideCharacter)
        }
    }

    /// The UTF-16 offset of the byte offset `byte`.
    pub fn byte_to_utf16(&self, byte: usize) -> Result<usize, PositionError> {
        Ok(self.rope.char_to_utf16_cu(self.char_at_byte(byte)?))
    }

    /// The byte offset of the UTF-16 offset `utf16`.
    pub fn utf16_to_byte(&self, utf16: usize) -> Result<usize, PositionError> {
        Ok(self.rope.char_to_byte(self.char_at_utf16(utf16)?))
    }

    /// The line and columns of the byte offset `byte`.
    pub fn point(&self, byte: usize) -> Result<Point, PositionError> {
        let char = self.char_at_byte(byte)?;
        let line = self.rope.char_to_line(char);
        let line_char = self.rope.line_to_char(line);
        Ok(Point {
            line,
            utf8: byte - self.rope.line_to_byte(line),
            utf16: self.rope.char_to_utf16_cu(char) - self.rope.char_to_utf16_cu(line_char),
            utf32: char - line_char,
        })
    }

    /// The byte offset of column `column`, counted in `unit`, on line `line`; the column may reach the end of the
    /// line but not into its line break.
    pub fn offset(&self, line: usize, column: usize, unit: Unit) -> Result<usize, PositionError> {
        if line >= self.line_count() {
            return Err(PositionError::OutOfBounds);
        }
        let start = self.rope.line_to_char(line);
        let end = self.content_end(line);
        let char = match unit {
            Unit::Utf32 => start.checked_add(column).filter(|&c| c <= end),
            Unit::Utf16 => {
                let target = self.rope.char_to_utf16_cu(start).saturating_add(column);
                if target > self.rope.char_to_utf16_cu(end) {
                    None
                } else {
                    Some(self.char_at_utf16(target)?)
                }
            }
            Unit::Utf8 => {
                let target = self.rope.char_to_byte(start).saturating_add(column);
                if target > self.rope.char_to_byte(end) {
                    None
                } else {
                    Some(self.char_at_byte(target)?)
                }
            }
        };
        char.map(|c| self.rope.char_to_byte(c))
            .ok_or(PositionError::OutOfBounds)
    }

    /// The byte offset of a language server's position, read as the protocol asks: a line beyond the text means its
    /// end, a column beyond the line means the end of the line, and a column inside a character means its start.
    pub fn lsp_offset(&self, line: u32, character: u32, unit: Unit) -> usize {
        let line = usize::try_from(line).unwrap_or(usize::MAX);
        if line >= self.line_count() {
            return self.len_bytes();
        }
        let column = usize::try_from(character).unwrap_or(usize::MAX);
        let start = self.rope.line_to_char(line);
        let end = self.content_end(line);
        let char = match unit {
            Unit::Utf32 => start.saturating_add(column).min(end),
            Unit::Utf16 => {
                let limit = self.rope.char_to_utf16_cu(end);
                let target = self.rope.char_to_utf16_cu(start).saturating_add(column).min(limit);
                self.rope.utf16_cu_to_char(target)
            }
            Unit::Utf8 => {
                let limit = self.rope.char_to_byte(end);
                let target = self.rope.char_to_byte(start).saturating_add(column).min(limit);
                self.rope.byte_to_char(target)
            }
        };
        self.rope.char_to_byte(char)
    }

    /// Whether the byte offset `byte` falls between the CR and the LF of one line break.
    pub(crate) fn inside_crlf(&self, byte: usize) -> bool {
        byte > 0 && byte < self.len_bytes() && self.rope.byte(byte - 1) == b'\r' && self.rope.byte(byte) == b'\n'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_convert_between_bytes_and_utf16_around_wide_characters() {
        let text = Text::new("aä😀b");
        assert_eq!(text.len_bytes(), 1 + 2 + 4 + 1);
        assert_eq!(text.len_utf16(), 1 + 1 + 2 + 1);
        assert_eq!(text.byte_to_utf16(7), Ok(4));
        assert_eq!(text.utf16_to_byte(4), Ok(7));
        assert_eq!(text.utf16_to_byte(3), Err(PositionError::InsideCharacter));
        assert_eq!(text.byte_to_utf16(2), Err(PositionError::InsideCharacter));
        assert_eq!(text.utf16_to_byte(6), Err(PositionError::OutOfBounds));
        assert_eq!(text.utf16_to_byte(5), Ok(8));
    }

    #[test]
    fn a_point_has_its_column_in_every_unit() {
        let text = Text::new("x\r\nä😀z\rlast");
        let z = text.point(3 + 2 + 4).unwrap();
        assert_eq!(
            z,
            Point {
                line: 1,
                utf8: 6,
                utf16: 3,
                utf32: 2
            }
        );
        assert_eq!(text.point(text.len_bytes()).unwrap().line, 2, "a lone CR ends a line");
        for unit in [Unit::Utf8, Unit::Utf16, Unit::Utf32] {
            assert_eq!(text.offset(1, z.column(unit), unit), Ok(9), "{unit:?}");
        }
    }

    #[test]
    fn a_column_may_reach_the_end_of_its_line_but_not_into_the_break() {
        let text = Text::new("ab\r\ncd");
        assert_eq!(text.offset(0, 2, Unit::Utf16), Ok(2));
        assert_eq!(text.offset(0, 3, Unit::Utf16), Err(PositionError::OutOfBounds));
        assert_eq!(text.offset(2, 0, Unit::Utf16), Err(PositionError::OutOfBounds));
        assert_eq!(text.line_content(0), Some(0..2));
        assert_eq!(text.line_bytes(0), Some(0..4));
        let wide = Text::new("😀");
        assert_eq!(wide.offset(0, 1, Unit::Utf16), Err(PositionError::InsideCharacter));
        assert_eq!(wide.offset(0, 2, Unit::Utf8), Err(PositionError::InsideCharacter));
        assert_eq!(wide.offset(0, 1, Unit::Utf32), Ok(4));
    }

    #[test]
    fn language_server_positions_beyond_a_line_or_the_text_are_clamped() {
        let text = Text::new("ab😀\ncd\n");
        assert_eq!(text.lsp_offset(0, 99, Unit::Utf16), 6);
        assert_eq!(
            text.lsp_offset(0, 3, Unit::Utf16),
            2,
            "inside the emoji means its start"
        );
        assert_eq!(text.lsp_offset(0, 3, Unit::Utf8), 2);
        assert_eq!(text.lsp_offset(1, 1, Unit::Utf32), 8);
        assert_eq!(text.lsp_offset(9, 0, Unit::Utf16), text.len_bytes());
    }

    #[test]
    fn only_lf_crlf_and_cr_break_lines() {
        let text = Text::new("a\u{2028}b\u{85}c\u{c}d\ne\r\nf\rg");
        assert_eq!(text.line_count(), 4);
        assert_eq!(text.line(0).as_deref(), Some("a\u{2028}b\u{85}c\u{c}d\n"));
        assert_eq!(text.line(3).as_deref(), Some("g"));
    }

    #[test]
    fn a_clone_is_a_copy_that_stays_as_it_was() {
        let mut text = Text::new("one");
        let copy = text.clone();
        text.rope_mut().insert(3, " two");
        assert_eq!(copy.to_string(), "one");
        assert_eq!(text.to_string(), "one two");
        assert_ne!(copy, text);
    }

    #[test]
    fn a_slice_must_lie_on_character_boundaries() {
        let text = Text::new("aä");
        assert_eq!(text.slice(1..3).as_deref(), Some("ä"));
        assert_eq!(text.slice(1..2), None);
        assert_eq!(text.slice(Range { start: 2, end: 1 }), None);
        assert_eq!(text.slice(0..9), None);
    }

    #[test]
    fn the_head_of_a_line_is_its_first_characters_or_all_of_a_shorter_one() {
        let text = Text::new(&format!("äb😀c\nxy\r\n{}", "z".repeat(100_000)));
        assert_eq!(text.line_head(0, 3).as_deref(), Some("äb😀"));
        assert_eq!(text.line_head(0, 99).as_deref(), Some("äb😀c\n"));
        assert_eq!(text.line_head(1, 3).as_deref(), Some("xy\r"));
        assert_eq!(text.line_head(2, 5).as_deref(), Some("zzzzz"));
        assert_eq!(text.line_head(2, 0).as_deref(), Some(""));
        assert_eq!(text.line_head(3, 5), None);
    }

    #[test]
    fn bytes_are_compared_across_pieces_and_only_on_character_boundaries() {
        let long = "ä".repeat(10_000);
        let ours = Text::new(&format!("a{long}z"));
        assert!(ours.chunks().count() > 1, "the text lies in several pieces");
        let end = ours.len_bytes() - 1;
        assert_eq!(ours.same_bytes(&Text::new(&format!("b{long}y")), 1..end), Some(true));
        assert_eq!(
            ours.same_bytes(&Text::new(&format!("a{long}y")), 0..end + 1),
            Some(false)
        );
        assert_eq!(ours.same_bytes(&ours, 2..3), None, "inside an ä");
        assert_eq!(ours.same_bytes(&Text::new("a"), 0..end), None, "beyond the other text");
        assert!(ours.has_at(1..end, &long));
        assert!(!ours.has_at(0..end, &long));
        assert!(!ours.has_at(2..3, ""), "inside an ä");
        assert!(!ours.has_at(end..end + 9, "z"), "beyond the text");
    }
}
