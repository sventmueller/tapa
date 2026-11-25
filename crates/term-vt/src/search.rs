//! Searching the screen and its history (F104): a regular expression or plain text, with or without case and at
//! word boundaries, over logical lines, so a match may span rows that wrapped. Matches are anchored in global line
//! numbers like a selection.

use regex::{Regex, RegexBuilder};

use crate::cell::CellFlags;
use crate::grid::Grid;
use crate::selection::Anchor;

/// How to search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    /// The pattern is a regular expression, else plain text.
    pub regex: bool,
    /// Upper and lower case count.
    pub case_sensitive: bool,
    /// Only whole words match.
    pub whole_word: bool,
}

/// A match, first to last cell inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// The first cell.
    pub start: Anchor,
    /// The last cell.
    pub end: Anchor,
}

/// The expression `pattern` stands for with `options`; an error for an invalid regular expression.
pub fn compile(pattern: &str, options: Options) -> Result<Regex, regex::Error> {
    let body = if options.regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    let body = if options.whole_word {
        format!(r"\b(?:{body})\b")
    } else {
        body
    };
    RegexBuilder::new(&body)
        .case_insensitive(!options.case_sensitive)
        .size_limit(1 << 20)
        .build()
}

/// Every match of `expression` in `grid`, oldest first; at most `limit`.
pub fn find_all(grid: &Grid, expression: &Regex, limit: usize) -> Vec<Match> {
    let mut found = Vec::new();
    let total = grid.history.len() + grid.lines();
    let mut index = 0;
    while index < total && found.len() < limit {
        let (text, cells, next) = logical_line(grid, index);
        for hit in expression.find_iter(&text) {
            if hit.start() == hit.end() {
                continue;
            }
            let start = cells
                .partition_point(|&(byte, _)| byte <= hit.start())
                .saturating_sub(1);
            let end = cells.partition_point(|&(byte, _)| byte < hit.end()).saturating_sub(1);
            if let (Some(&(_, start)), Some(&(_, end))) = (cells.get(start), cells.get(end)) {
                found.push(Match {
                    start,
                    end: with_spacer(grid, end),
                });
                if found.len() >= limit {
                    break;
                }
            }
        }
        index = next;
    }
    found
}

/// `at`, or the spacer after it when it holds a wide character, so that a match covers both its cells.
fn with_spacer(grid: &Grid, at: Anchor) -> Anchor {
    let spacer = grid
        .row_global(at.line)
        .and_then(|row| row.cells.get(at.column + 1).copied())
        .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE_SPACER));
    if spacer {
        Anchor {
            column: at.column + 1,
            ..at
        }
    } else {
        at
    }
}

/// The text of the logical line that starts at row `index` from the top of the history, with the byte offset and
/// cell of every character, and the index of the row after it.
fn logical_line(grid: &Grid, mut index: usize) -> (String, Vec<(usize, Anchor)>, usize) {
    let mut text = String::new();
    let mut cells = Vec::new();
    let total = grid.history.len() + grid.lines();
    while index < total {
        let Some(row) = grid.row_in_all(index) else { break };
        let line = grid.global(index);
        for (column, cell) in row.cells.iter().enumerate() {
            if cell
                .flags
                .intersects(CellFlags(CellFlags::WIDE_SPACER.0 | CellFlags::LEADING_WIDE_SPACER.0))
            {
                continue;
            }
            cells.push((text.len(), Anchor { line, column }));
            text.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(extra) = cell
                .flags
                .contains(CellFlags::EXTRA)
                .then(|| row.extra(column))
                .flatten()
            {
                text.push_str(&extra.zero_width);
            }
        }
        index += 1;
        if !row.wrapped() {
            break;
        }
    }
    (text, cells, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::{Size, Terminal};

    #[test]
    fn matches_are_found_across_wrapped_rows_and_with_options() {
        let mut terminal = Terminal::new(Size { columns: 6, lines: 3 }, 100);
        terminal.advance(b"error here\r\nError again\r\nterror\r\n");
        let grid = terminal.grid();
        let plain = compile("error", Options::default()).unwrap();
        let all = find_all(grid, &plain, 100);
        assert_eq!(all.len(), 3);
        assert_eq!((all[0].start.column, all[0].end.column), (0, 4));
        let words = compile(
            "error",
            Options {
                whole_word: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(find_all(grid, &words, 100).len(), 2);
        let exact = compile(
            "Error",
            Options {
                case_sensitive: true,
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(find_all(grid, &exact, 100).len(), 1);
        let spanning = compile("rror h", Options::default()).unwrap();
        let hit = find_all(grid, &spanning, 100)[0];
        assert_ne!(hit.start.line, hit.end.line, "the match wraps");
        assert!(
            compile(
                "(",
                Options {
                    regex: true,
                    ..Options::default()
                }
            )
            .is_err()
        );
    }
}
