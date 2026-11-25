//! Typing help as edits for the view to make: brackets and quotes closed as they are typed and stepped over, a new
//! line indented like the one above and lines indented anew by the same rule, Tab over several lines, and lines
//! commented out and in again.
//!
//! Every function works on a piece of the text and a selection in it, both in AppKit's UTF-16 units, and returns the
//! `Step` the view makes. The piece may be the whole text; what it must hold at least is said at each function. Lines
//! end at LF, CRLF and a lone CR, as for the document. With several carets the view asks once per caret, from the
//! last to the first, so the edits at the later carets do not move the earlier ones.

use std::ops::Range;

/// What a key does to the text, in UTF-16 units of the piece it was worked out on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Only the caret moves, to this offset.
    Move(usize),
    /// `text` goes in as typed over the selection; then `select` is selected, or without it the caret stays after
    /// the text.
    Type {
        /// What goes in.
        text: String,
        /// What is selected after it.
        select: Option<Range<usize>>,
    },
    /// `range` becomes `text`, as an undo step of its own, and the new text is selected.
    Replace {
        /// What is replaced.
        range: Range<usize>,
        /// What it becomes.
        text: String,
    },
}

/// Pairs typed together; the closing one is stepped over when typed in front of itself.
const PAIRS: [(char, char); 6] = [('(', ')'), ('[', ']'), ('{', '}'), ('"', '"'), ('\'', '\''), ('`', '`')];

/// What typing `typed` over `selection` of `text` does, unless it goes in as it is: a closing bracket or quote in
/// front of itself is stepped over, an opening one wraps a selection or gets its closing one where nothing follows
/// that it would belong to, and a quote right after a letter or another quote stays single.
///
/// `text` holds at least the character before the selection, the selection and the character after it.
pub fn typed(text: &str, selection: Range<usize>, typed: char) -> Option<Step> {
    let (start, end) = (byte_of(text, selection.start), byte_of(text, selection.end));
    let next = text[end..].chars().next();
    if selection.is_empty() && next == Some(typed) && PAIRS.iter().any(|(_, close)| *close == typed) {
        return Some(Step::Move(selection.start + 1));
    }
    let (open, close) = PAIRS.iter().find(|(open, _)| *open == typed).copied()?;
    if !selection.is_empty() {
        return Some(Step::Type {
            text: format!("{open}{}{close}", &text[start..end]),
            select: Some(selection.start + 1..selection.end + 1),
        });
    }
    let free_after = next.is_none_or(|n| n.is_whitespace() || ")]},;".contains(n));
    let previous = text[..start].chars().next_back();
    let quote = open == close;
    if !free_after || (quote && previous.is_some_and(|p| p.is_alphanumeric() || p == open)) {
        return None;
    }
    let caret = selection.start + 1;
    Some(Step::Type {
        text: format!("{open}{close}"),
        select: Some(caret..caret),
    })
}

/// What Return over `selection` of `text` does: a new line with `ending`, indented like the line above, one `unit`
/// deeper after an opening bracket or a colon; between a bracket pair the closing one goes to a line of its own.
///
/// `text` holds at least the selection's line up to its start, the selection and the character after it.
pub fn newline(text: &str, selection: Range<usize>, ending: &str, unit: &str) -> Step {
    let start = byte_of(text, selection.start);
    let line_start = text[..start].rfind(['\n', '\r']).map_or(0, |at| at + 1);
    let before = &text[line_start..start];
    let indent: String = before.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let opens = before.trim_end().ends_with(['{', '(', '[', ':']);
    let next = text[byte_of(text, selection.end)..].chars().next();
    let closes = next.is_some_and(|n| "}])".contains(n));
    if opens && closes {
        let typed = format!("{ending}{indent}{unit}{ending}{indent}");
        let caret = selection.start + utf16_len(&typed) - utf16_len(&indent) - utf16_len(ending);
        Step::Type {
            text: typed,
            select: Some(caret..caret),
        }
    } else if opens {
        Step::Type {
            text: format!("{ending}{indent}{unit}"),
            select: None,
        }
    } else {
        Step::Type {
            text: format!("{ending}{indent}"),
            select: None,
        }
    }
}

/// What Tab over `selection` of `text` does: one `unit` in place of a selection within a line, else each selected
/// line indented by `unit` but the blank ones.
///
/// `text` holds at least the whole lines the selection touches.
pub fn tab(text: &str, selection: Range<usize>, unit: &str) -> Step {
    let selected = &text[byte_of(text, selection.start)..byte_of(text, selection.end)];
    if !selected.contains(['\n', '\r']) {
        return Step::Type {
            text: unit.to_string(),
            select: None,
        };
    }
    let lines = lines_of(text, &selection);
    let indented: String = split_lines(&text[lines.clone()])
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("{unit}{line}")
            }
        })
        .collect();
    Step::Replace {
        range: utf16_range(text, lines),
        text: indented,
    }
}

/// What Vim's `=` makes of the lines `selection` touches in `text`, by the rule of `newline`: each line indented like
/// the line above it that is not blank, one `unit` deeper after an opening bracket or a colon, and one `unit` less
/// where it starts with a closing bracket; blank lines lose their white space.
///
/// `text` holds at least the whole lines the selection touches and those before them up to one that is not blank.
pub fn reindent(text: &str, selection: Range<usize>, unit: &str) -> Step {
    let lines = lines_of(text, &selection);
    let mut above = split_lines(&text[..lines.start])
        .map(|line| line.trim_end_matches(['\n', '\r']))
        .filter(|line| !line.trim().is_empty())
        .last()
        .map(str::to_string);
    let mut indented = String::new();
    for line in split_lines(&text[lines.clone()]) {
        let content = line.trim_end_matches(['\n', '\r']);
        let (body, ending) = (content.trim_start(), &line[content.len()..]);
        if body.is_empty() {
            indented.push_str(ending);
            continue;
        }
        let mut indent = above.as_deref().map(indentation).unwrap_or_default().to_string();
        if above
            .as_deref()
            .is_some_and(|above| above.trim_end().ends_with(['{', '(', '[', ':']))
        {
            indent.push_str(unit);
        }
        if body.starts_with(['}', ']', ')']) {
            indent = one_level_less(&indent, unit);
        }
        let line = format!("{indent}{body}");
        indented.push_str(&line);
        indented.push_str(ending);
        above = Some(line);
    }
    Step::Replace {
        range: utf16_range(text, lines),
        text: indented,
    }
}

/// `indent` without its last level: one `unit`, a tab, or as many spaces as `unit` is wide.
fn one_level_less(indent: &str, unit: &str) -> String {
    if let Some(less) = indent.strip_suffix(unit).or_else(|| indent.strip_suffix('\t')) {
        return less.to_string();
    }
    let spaces = indent.len() - indent.trim_end_matches(' ').len();
    indent[..indent.len() - spaces.min(unit.chars().count().max(1))].to_string()
}

/// What commenting the lines of `selection` in `text` out with the line comment `marker` does, or in again when every
/// one of them but the blank ones is a comment. Comments out at the indentation of the least indented line, with a
/// space after the marker, which commenting in takes away again.
///
/// `text` holds at least the whole lines the selection touches.
pub fn toggle_comment(text: &str, selection: Range<usize>, marker: &str) -> Step {
    let lines = lines_of(text, &selection);
    let content: Vec<&str> = split_lines(&text[lines.clone()]).collect();
    let written = || content.iter().filter(|line| !line.trim().is_empty());
    let commented = written().all(|line| line.trim_start().starts_with(marker));
    let indent = written()
        .map(|line| indentation(line).chars().count())
        .min()
        .unwrap_or(0);
    let changed: String = content
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                (*line).to_string()
            } else if commented {
                let start = indentation(line).len();
                let rest = &line[start + marker.len()..];
                format!("{}{}", &line[..start], rest.strip_prefix(' ').unwrap_or(rest))
            } else {
                let at = line.char_indices().nth(indent).map_or(line.len(), |(at, _)| at);
                format!("{}{marker} {}", &line[..at], &line[at..])
            }
        })
        .collect();
    Step::Replace {
        range: utf16_range(text, lines),
        text: changed,
    }
}

/// The white space a line starts with.
fn indentation(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// The lines of `text`, each with its line break.
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = line_end(rest, 0);
        let (line, after) = rest.split_at(end);
        rest = after;
        Some(line)
    })
}

/// The byte range of the whole lines `selection` touches, as `NSString`'s `lineRangeForRange` gives it: with their
/// line breaks, and without the line a selection only reaches the start of.
fn lines_of(text: &str, selection: &Range<usize>) -> Range<usize> {
    let start = byte_of(text, selection.start);
    let first = text[..start].rfind(['\n', '\r']).map_or(0, |at| at + 1);
    let last = if selection.is_empty() {
        start
    } else {
        byte_of(text, selection.end - 1)
    };
    first..line_end(text, last)
}

/// The byte offset right after the line break of the line holding byte `at`, or the text's end.
fn line_end(text: &str, at: usize) -> usize {
    let Some(found) = text[at..].find(['\n', '\r']).map(|found| at + found) else {
        return text.len();
    };
    if text[found..].starts_with("\r\n") {
        found + 2
    } else {
        found + 1
    }
}

/// The byte offset of UTF-16 offset `utf16`, at the start of the character it falls into and at most the text's end.
fn byte_of(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        let next = units + c.len_utf16();
        if next > utf16 {
            return at;
        }
        units = next;
    }
    text.len()
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn utf16_range(text: &str, bytes: Range<usize>) -> Range<usize> {
    let start = utf16_len(&text[..bytes.start]);
    start..start + utf16_len(&text[bytes])
}

#[cfg(test)]
mod tests;
