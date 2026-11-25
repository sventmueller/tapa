//! Folds by indentation: a line folds the lines below it that are indented deeper.

use crate::text::Text;

/// Lines that fold together: `start` stays visible, the lines after it up to `end` hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// The line that stays visible.
    pub start: usize,
    /// The last line that hides.
    pub end: usize,
}

/// The folds of `text`, ordered by their first line, with a tab reaching the next multiple of `tab_width`. Blank lines
/// belong to the fold around them, but no fold ends in one.
#[cfg(test)]
pub(crate) fn indentation_folds(text: &Text, tab_width: usize) -> Vec<Fold> {
    let tab_width = tab_width.max(1);
    folds_of(text.rope().lines().map(|line| indentation(line.chars(), tab_width)))
}

/// The folds of lines with the indentation `indents`, `None` for a blank line, as `indentation_folds` finds them.
fn folds_of(indents: impl Iterator<Item = Option<usize>>) -> Vec<Fold> {
    let mut folds = Vec::new();
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_content = 0;
    for (n, indent) in indents.enumerate() {
        let Some(indent) = indent else {
            continue;
        };
        while let Some(&(depth, start)) = open.last()
            && depth >= indent
        {
            open.pop();
            if last_content > start {
                folds.push(Fold {
                    start,
                    end: last_content,
                });
            }
        }
        open.push((indent, n));
        last_content = n;
    }
    for (_, start) in open.into_iter().rev() {
        if last_content > start {
            folds.push(Fold {
                start,
                end: last_content,
            });
        }
    }
    folds.sort_by_key(|fold| fold.start);
    folds
}

/// The indentation of every line, kept across edits, so that the folds after an edit look again only at the lines it
/// touched.
#[derive(Debug, Clone, Default)]
pub(crate) struct Indents {
    tab_width: usize,
    /// Each line's column of content, `Some(None)` for a blank line, `None` until looked at.
    lines: Vec<Option<Option<usize>>>,
}

impl Indents {
    /// The lines of `edits` were replaced, one edit after the other.
    pub(crate) fn edited(&mut self, edits: &[super::LineEdit]) {
        super::splice_lines(&mut self.lines, edits);
    }

    /// The folds of `text`, as `indentation_folds` finds them.
    pub(crate) fn folds(&mut self, text: &Text, tab_width: usize) -> Vec<Fold> {
        let tab_width = tab_width.max(1);
        if self.tab_width != tab_width || self.lines.len() != text.line_count() {
            self.tab_width = tab_width;
            self.lines = vec![None; text.line_count()];
        }
        for (n, indent) in self.lines.iter_mut().enumerate() {
            if indent.is_none() {
                *indent = Some(
                    text.rope()
                        .get_line(n)
                        .and_then(|line| indentation(line.chars(), tab_width)),
                );
            }
        }
        folds_of(self.lines.iter().map(|indent| indent.flatten()))
    }
}

/// The column the content of a line starts at, or `None` for a line of nothing but blanks.
fn indentation(chars: impl Iterator<Item = char>, tab_width: usize) -> Option<usize> {
    let mut column = 0;
    for ch in chars {
        match ch {
            ' ' => column += 1,
            '\t' => column += tab_width - column % tab_width,
            '\r' | '\n' => return None,
            _ => return Some(column),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(text: &str) -> Vec<(usize, usize)> {
        indentation_folds(&Text::new(text), 4)
            .into_iter()
            .map(|f| (f.start, f.end))
            .collect()
    }

    #[test]
    fn deeper_lines_fold_into_the_line_above_them() {
        let text = "fn a() {\n    if x {\n        y();\n    }\n\n    z();\n}\nfn b() {}\n";
        assert_eq!(folds(text), [(0, 5), (1, 2)]);
    }

    #[test]
    fn folds_kept_across_edits_are_those_of_the_text_after_them() {
        let mut text = Text::new("fn a() {\n    if x {\n        y();\n    }\n}\nb\n");
        let mut indents = Indents::default();
        assert_eq!(indents.folds(&text, 4), indentation_folds(&text, 4));
        for (line, typed) in [(2, "\n    w();"), (0, "x\n  "), (5, "\n\n")] {
            let at = text.line_content(line).map_or(text.len_bytes(), |content| content.end);
            let added = typed.matches('\n').count();
            let rope = text.rope_mut();
            let char_at = rope.byte_to_char(at);
            rope.insert(char_at, typed);
            indents.edited(&[crate::syntax::LineEdit {
                first: line,
                removed: 1,
                added: 1 + added,
            }]);
            assert_eq!(indents.folds(&text, 4), indentation_folds(&text, 4), "after {typed:?}");
        }
    }

    #[test]
    fn trailing_blank_lines_stay_outside_and_tabs_reach_the_next_stop() {
        assert_eq!(folds("a:\n\tb\n  \n\nc\n"), [(0, 1)]);
        assert_eq!(
            folds("a\n \tb\n    c\n"),
            [(0, 2)],
            "a blank and a tab reach the column of four blanks"
        );
        assert_eq!(
            folds("x\n  y\n    z"),
            [(0, 2), (1, 2)],
            "folds still open at the end close there"
        );
        assert!(folds("").is_empty() && folds("\n\n").is_empty());
    }
}
