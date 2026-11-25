//! What a ⌘-click on the screen points at: a link the program marked (OSC 8), an address in the text, or a word that
//! may name a file.

use term_session::{Cell, CellFlags, Point};

/// The text of the line through `point`, rows joined where the line wrapped, with the screen cell of each character.
pub(crate) struct Line {
    pub(crate) chars: Vec<char>,
    pub(crate) cells: Vec<Point>,
}

impl Line {
    pub(crate) fn around(rows: &[Vec<Cell>], point: Point) -> Line {
        let wrapped = |line: usize| {
            rows.get(line)
                .and_then(|row| row.last())
                .is_some_and(|cell| cell.flags.contains(CellFlags::WRAPPED))
        };
        let mut first = point.line;
        while first > 0 && wrapped(first - 1) {
            first -= 1;
        }
        let mut last = point.line;
        while last + 1 < rows.len() && wrapped(last) {
            last += 1;
        }
        let mut line = Line {
            chars: vec![],
            cells: vec![],
        };
        for (index, row) in rows.iter().enumerate().take(last + 1).skip(first) {
            for (column, cell) in row.iter().enumerate() {
                if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                    line.chars.push(cell.ch);
                    line.cells.push(Point { line: index, column });
                }
            }
        }
        line
    }

    /// The index of the character drawn in `point`, which for a wide character is either of its cells.
    pub(crate) fn index_of(&self, point: Point) -> Option<usize> {
        self.cells
            .iter()
            .rposition(|cell| cell.line == point.line && cell.column <= point.column)
    }

    fn text(&self, start: usize, end: usize) -> String {
        self.chars[start..end].iter().collect()
    }
}

/// The address at `point`: the target of a link the program marked, else a URL written in the text.
pub(crate) fn url_at(rows: &[Vec<Cell>], point: Point) -> Option<String> {
    if let Some(link) = &rows.get(point.line)?.get(point.column)?.hyperlink {
        return Some(link.to_string());
    }
    let line = Line::around(rows, point);
    let at = line.index_of(point)?;
    let (start, end) = urls(&line.chars)
        .into_iter()
        .find(|&(start, end)| (start..end).contains(&at))?;
    Some(line.text(start, end))
}

/// Where URLs such as `https://example.com/a?b=c` start and end in `chars`.
pub(crate) fn urls(chars: &[char]) -> Vec<(usize, usize)> {
    let mut found = vec![];
    let mut from = 0;
    while let Some(colon) =
        (from..chars.len().saturating_sub(2)).find(|&i| chars[i] == ':' && chars[i + 1] == '/' && chars[i + 2] == '/')
    {
        from = colon + 3;
        let mut start = colon;
        while start > 0 && (chars[start - 1].is_ascii_alphanumeric() || "+.-".contains(chars[start - 1])) {
            start -= 1;
        }
        while start < colon && !chars[start].is_ascii_alphabetic() {
            start += 1;
        }
        if start == colon {
            continue;
        }
        let mut end = from;
        while end < chars.len() && !ends_url(chars[end]) {
            end += 1;
        }
        let end = trim_trailing(chars, start, end);
        if end > from {
            found.push((start, end));
            from = end;
        }
    }
    found
}

fn ends_url(c: char) -> bool {
    c.is_whitespace() || c.is_control() || "<>\"'`{}|\\^".contains(c)
}

/// `end` without punctuation that closes the sentence or a bracket opened before the URL.
fn trim_trailing(chars: &[char], start: usize, mut end: usize) -> usize {
    let count = |c: char| chars[start..end].iter().filter(|&&x| x == c).count();
    // Only closing brackets and punctuation are cut, so the opening brackets stay as they are counted here.
    let (round, square) = (count('('), count('['));
    let (mut closed_round, mut closed_square) = (count(')'), count(']'));
    while end > start {
        let last = chars[end - 1];
        if last == ')' && round < closed_round {
            closed_round -= 1;
        } else if last == ']' && square < closed_square {
            closed_square -= 1;
        } else if !".,;:!?".contains(last) {
            break;
        }
        end -= 1;
    }
    end
}

/// A file a program printed, with the line and column after it, as in `src/main.rs:12:5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileAt {
    pub(crate) path: String,
    /// The line, from 1.
    pub(crate) line: Option<usize>,
    /// The column, from 1.
    pub(crate) column: Option<usize>,
}

/// The word at `point` read as a path with an optional line and column.
pub(crate) fn path_at(rows: &[Vec<Cell>], point: Point) -> Option<FileAt> {
    let line = Line::around(rows, point);
    let at = line.index_of(point)?;
    let breaks = |c: char| c.is_whitespace() || c.is_control() || "\"'`()<>[]{}|;".contains(c);
    if breaks(line.chars[at]) {
        return None;
    }
    let mut start = at;
    while start > 0 && !breaks(line.chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < line.chars.len() && !breaks(line.chars[end]) {
        end += 1;
    }
    let word = line.text(start, end);
    let mut path = word.trim_end_matches(['.', ',', ':']);
    let mut numbers = vec![];
    while numbers.len() < 2
        && let Some((before, number)) = path.rsplit_once(':')
        && let Ok(number) = number.parse::<usize>()
    {
        numbers.insert(0, number);
        path = before;
    }
    (!path.is_empty()).then(|| FileAt {
        path: path.to_string(),
        line: numbers.first().copied(),
        column: numbers.get(1).copied(),
    })
}

#[cfg(test)]
mod tests {
    use term_session::Rgb;

    use super::*;

    fn rows(lines: &[&str], wrapped: &[usize]) -> Vec<Vec<Cell>> {
        let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        lines
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let mut chars: Vec<char> = text.chars().collect();
                chars.resize(width, ' ');
                let last = chars.len() - 1;
                chars
                    .into_iter()
                    .enumerate()
                    .map(|(column, ch)| Cell {
                        ch,
                        zero_width: vec![],
                        fg: Rgb::default(),
                        bg: Rgb::default(),
                        underline_color: None,
                        flags: if column == last && wrapped.contains(&index) {
                            CellFlags::WRAPPED
                        } else {
                            CellFlags::default()
                        },
                        hyperlink: None,
                    })
                    .collect()
            })
            .collect()
    }

    fn at(line: usize, column: usize) -> Point {
        Point { line, column }
    }

    #[test]
    fn a_url_in_the_text_is_found_without_the_punctuation_around_it() {
        let screen = rows(&["see (https://example.com/a_(b)?q=1). ok"], &[]);
        assert_eq!(
            url_at(&screen, at(0, 10)).as_deref(),
            Some("https://example.com/a_(b)?q=1")
        );
        assert_eq!(url_at(&screen, at(0, 1)), None);
        let local = rows(&["  ➜  Local:   http://localhost:5173/"], &[]);
        assert_eq!(url_at(&local, at(0, 20)).as_deref(), Some("http://localhost:5173/"));
    }

    #[test]
    fn closing_brackets_without_an_opening_one_are_cut_however_many_follow() {
        let line: Vec<char> = format!("x https://a.example/(b)[c]{}.", ")]".repeat(50_000))
            .chars()
            .collect();
        assert_eq!(urls(&line), [(2, 26)]);
        let screen = rows(&["(see https://a.example/w_(x)).)"], &[]);
        assert_eq!(url_at(&screen, at(0, 8)).as_deref(), Some("https://a.example/w_(x)"));
    }

    #[test]
    fn a_url_that_wraps_onto_the_next_row_is_one_url() {
        let screen = rows(&["go https://exa", "mple.com/x now"], &[0]);
        assert_eq!(url_at(&screen, at(1, 2)).as_deref(), Some("https://example.com/x"));
        assert_eq!(url_at(&screen, at(0, 5)).as_deref(), Some("https://example.com/x"));
    }

    #[test]
    fn a_link_the_program_marked_wins() {
        let mut screen = rows(&["click here"], &[]);
        screen[0][7].hyperlink = Some("https://marked.example/".into());
        assert_eq!(url_at(&screen, at(0, 7)).as_deref(), Some("https://marked.example/"));
    }

    #[test]
    fn a_path_keeps_its_line_and_column_apart() {
        let screen = rows(
            &[
                "error: src/main.rs:12:5: oops",
                "--> crates/x/lib.rs:3",
                "see README.md.",
            ],
            &[],
        );
        let file = |path: &str, line, column| {
            Some(FileAt {
                path: path.into(),
                line,
                column,
            })
        };
        assert_eq!(path_at(&screen, at(0, 9)), file("src/main.rs", Some(12), Some(5)));
        assert_eq!(path_at(&screen, at(1, 8)), file("crates/x/lib.rs", Some(3), None));
        assert_eq!(path_at(&screen, at(2, 6)), file("README.md", None, None));
        assert_eq!(path_at(&screen, at(0, 6)), None, "a blank");
    }
}
