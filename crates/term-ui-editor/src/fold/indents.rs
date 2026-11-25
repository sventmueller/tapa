//! The indentation of each line, looked at only when asked and kept across edits from the changed lines on (SN-T2),
//! so the line numbers find the folds by indentation around what is on screen without going through the whole text. They agree with `Document::folds`.

use term_document::Text;

/// How far into a line its indentation is looked for before the whole line is read.
const HEAD: usize = 256;

/// Each line's column of content, `Some(None)` for a blank line, `None` until looked at.
#[derive(Debug, Default)]
pub(crate) struct Indents {
    tab_width: usize,
    lines: Vec<Option<Option<usize>>>,
}

impl Indents {
    /// Lines `first..=old_last` were replaced by `first..=new_last`.
    pub(crate) fn edited(&mut self, first: usize, old_last: usize, new_last: usize) {
        if first >= self.lines.len() {
            self.lines.clear();
            return;
        }
        let old_end = (old_last + 1).min(self.lines.len());
        self.lines
            .splice(first..old_end, std::iter::repeat_n(None, new_last + 1 - first));
    }

    /// The column line `line` of `text` starts its content at, `None` for a blank line or one past the end.
    pub(crate) fn get(&mut self, text: &Text, line: usize, tab_width: usize) -> Option<usize> {
        let tab_width = tab_width.max(1);
        if self.tab_width != tab_width || self.lines.len() != text.line_count() {
            self.tab_width = tab_width;
            self.lines = vec![None; text.line_count()];
        }
        let slot = self.lines.get_mut(line)?;
        *slot.get_or_insert_with(|| indentation(text, line, tab_width))
    }

    /// Whether line `line` starts a fold: the next line with content is indented deeper.
    pub(crate) fn folds_at(&mut self, text: &Text, line: usize, tab_width: usize) -> bool {
        let Some(indent) = self.get(text, line, tab_width) else {
            return false;
        };
        (line + 1..text.line_count())
            .find_map(|next| self.get(text, next, tab_width))
            .is_some_and(|next| next > indent)
    }
}

/// The column the content of line `line` starts at, with a tab reaching the next multiple of `tab_width`, or `None`
/// for a line of nothing but blanks.
fn indentation(text: &Text, line: usize, tab_width: usize) -> Option<usize> {
    let head = text.line_head(line, HEAD)?;
    match column(head.chars(), tab_width) {
        Column::Content(column) => Some(column),
        Column::Blank => None,
        Column::Open => text
            .line(line)
            .and_then(|whole| match column(whole.chars(), tab_width) {
                Column::Content(column) => Some(column),
                Column::Blank | Column::Open => None,
            }),
    }
}

enum Column {
    Content(usize),
    Blank,
    /// Nothing but blanks, without the line's end.
    Open,
}

fn column(chars: impl Iterator<Item = char>, tab_width: usize) -> Column {
    let mut column = 0;
    for ch in chars {
        match ch {
            ' ' => column += 1,
            '\t' => column += tab_width - column % tab_width,
            '\r' | '\n' => return Column::Blank,
            _ => return Column::Content(column),
        }
    }
    Column::Open
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "fn a() {\n    if x {\n        y();\n\n        z();\n    }\n}\n\nfn b() {\n\tc();\n}\n  \n";

    /// The folds by indentation as the document finds them, first line and last hidden line.
    fn folds(text: &str) -> Vec<(usize, usize)> {
        let mut document = term_document::Document::untitled(text);
        document
            .folds(4)
            .into_iter()
            .filter(|f| f.end > f.start)
            .map(|f| (f.start, f.end))
            .collect()
    }

    #[test]
    fn the_lines_that_fold_are_those_the_document_folds() {
        let text = Text::new(SOURCE);
        let mut indents = Indents::default();
        let starts: Vec<usize> = (0..text.line_count())
            .filter(|&line| indents.folds_at(&text, line, 4))
            .collect();
        let expected: Vec<usize> = folds(SOURCE).into_iter().map(|(start, _)| start).collect();
        assert_eq!(starts, expected);
    }

    #[test]
    fn an_edit_looks_again_only_at_the_lines_it_replaced() {
        let mut indents = Indents::default();
        let before = Text::new("a\n  b\n  c\nd\n");
        assert_eq!(indents.get(&before, 1, 4), Some(2));
        assert_eq!(indents.get(&before, 2, 4), Some(2));
        assert_eq!(indents.get(&before, 3, 4), Some(0));
        let after = Text::new("a\n  b\n    x\n    y\n  c\nd\n");
        indents.edited(1, 1, 3);
        assert_eq!(
            indents.lines[4],
            Some(Some(2)),
            "the lines after the edit kept what was found"
        );
        assert_eq!(indents.lines[5], Some(Some(0)));
        assert_eq!(indents.lines[1], None, "the edited line is looked at again");
        assert_eq!(indents.lines[2], None);
        assert_eq!(indents.get(&after, 2, 4), Some(4));
    }

    #[test]
    fn a_blank_head_longer_than_looked_at_reads_the_whole_line() {
        let text = Text::new(&format!("{}x\n", " ".repeat(HEAD + 10)));
        assert_eq!(Indents::default().get(&text, 0, 4), Some(HEAD + 10));
    }
}
