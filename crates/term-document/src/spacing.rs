//! The whitespace of a text: how its lines indent, guessed from them; and what a save trims, adds or converts, as
//! UTF-16 ranges for the view to edit.

use std::ops::Range;

use crate::document::LineEnding;

/// How the lines of a text indent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// One tab per level.
    Tabs,
    /// This many spaces per level.
    Spaces(usize),
}

/// Lines a guess looks at, which bounds the work on a huge file.
const SAMPLE: usize = 10_000;

/// Steps between the indentation of neighbouring lines that count as a level; one column is alignment.
const STEPS: Range<usize> = 2..9;

/// The indentation most indented lines of `text` use, or `None` when no line is indented.
pub fn detect_indent(text: &str) -> Option<Indent> {
    let (mut tabs, mut spaces) = (0usize, 0usize);
    let mut steps = [0usize; STEPS.end];
    let mut narrowest = usize::MAX;
    let mut previous = 0usize;
    for line in text.lines().take(SAMPLE) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') {
            tabs += 1;
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > 0 {
            spaces += 1;
            narrowest = narrowest.min(indent);
        }
        let step = indent.abs_diff(previous);
        if STEPS.contains(&step) {
            steps[step] += 1;
        }
        previous = indent;
    }
    if tabs == 0 && spaces == 0 {
        return None;
    }
    if tabs > spaces {
        return Some(Indent::Tabs);
    }
    // A dedent by two levels steps twice as far, so on a tie the narrower step is the level.
    let width = STEPS
        .rev()
        .max_by_key(|&step| steps[step])
        .filter(|&step| steps[step] > 0)
        .unwrap_or_else(|| narrowest.clamp(STEPS.start, STEPS.end - 1));
    Some(Indent::Spaces(width))
}

/// The spaces and tabs at the end of each line of `text`, as UTF-16 ranges.
pub fn trailing_whitespace(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut run: Option<usize> = None;
    let mut at = 0usize;
    for c in text.chars() {
        match c {
            ' ' | '\t' => {
                run.get_or_insert(at);
            }
            '\n' | '\r' => {
                if let Some(start) = run.take() {
                    found.push(start..at);
                }
            }
            _ => run = None,
        }
        at += c.len_utf16();
    }
    if let Some(start) = run {
        found.push(start..at);
    }
    found
}

/// Whether `text` has text after its last line break.
pub fn lacks_final_newline(text: &str) -> bool {
    !text.is_empty() && !text.ends_with(['\n', '\r'])
}

/// The line breaks of `text` that are not `wanted`, as UTF-16 ranges.
pub fn other_breaks(text: &str, wanted: LineEnding) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut at = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let kind = match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                LineEnding::Crlf
            }
            '\r' => LineEnding::Cr,
            '\n' => LineEnding::Lf,
            _ => {
                at += c.len_utf16();
                continue;
            }
        };
        let length = kind.as_str().len();
        if kind != wanted {
            found.push(at..at + length);
        }
        at += length;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indentation_is_guessed_from_the_steps_between_lines() {
        let four = "fn a() {\n    if x {\n        y();\n    }\n}\n";
        assert_eq!(detect_indent(four), Some(Indent::Spaces(4)));
        let two = "a:\n  b:\n    c: 1\n  d: 2\ne: 3\n";
        assert_eq!(detect_indent(two), Some(Indent::Spaces(2)));
        let dedent = "a {\n  b {\n    c\n  }\n}\nd {\n  e {\n    f\n}\n";
        assert_eq!(
            detect_indent(dedent),
            Some(Indent::Spaces(2)),
            "dedents of two levels do not make the level wider"
        );
        assert_eq!(detect_indent("a\n\tb\n\t\tc\n"), Some(Indent::Tabs));
        assert_eq!(detect_indent("no indentation\nat all\n"), None);
        let aligned = "/**\n * doc\n */\nfn a() {\n   b();\n}\n";
        assert_eq!(
            detect_indent(aligned),
            Some(Indent::Spaces(3)),
            "a single column is alignment"
        );
    }

    #[test]
    fn trailing_whitespace_ends_before_each_line_break() {
        assert_eq!(
            trailing_whitespace("a  \nb\t\r\nc \u{e4} \n  "),
            vec![1..3, 5..6, 11..12, 13..15]
        );
        assert_eq!(
            trailing_whitespace("\u{1F600} \n"),
            vec![2..3],
            "offsets count UTF-16 units"
        );
        assert!(trailing_whitespace("clean\ntext\n").is_empty());
    }

    #[test]
    fn a_final_newline_is_missing_only_after_text() {
        assert!(lacks_final_newline("a"));
        assert!(!lacks_final_newline("a\n"));
        assert!(!lacks_final_newline("a\r"));
        assert!(!lacks_final_newline(""));
    }

    #[test]
    fn breaks_of_another_kind_are_found() {
        let text = "a\r\nb\nc\rd\r\n";
        assert_eq!(other_breaks(text, LineEnding::Lf), vec![1..3, 6..7, 8..10]);
        assert_eq!(other_breaks(text, LineEnding::Crlf), vec![4..5, 6..7]);
        assert_eq!(other_breaks("\u{e4}\n", LineEnding::Crlf), vec![1..2]);
    }
}
