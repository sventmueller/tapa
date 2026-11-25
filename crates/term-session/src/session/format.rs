//! The selection with its colours and type styles (F114), for a copy that keeps the formatting.

use term_base::LockIgnoringPoison;

use super::Session;
use crate::snapshot::Rgb;

/// How a run of selected text looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Look {
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Characters next to each other that look the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledRun {
    /// The characters.
    pub text: String,
    /// How they look.
    pub look: Look,
}

/// Selected text in runs, line by line, without the trailing blanks of a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledText {
    /// The lines, a newline between each two.
    pub lines: Vec<Vec<StyledRun>>,
    /// The terminal's text colour.
    pub foreground: Rgb,
    /// The terminal's background.
    pub background: Rgb,
}

impl StyledText {
    /// The text without its looks.
    pub fn plain(&self) -> String {
        let lines: Vec<String> = self
            .lines
            .iter()
            .map(|runs| runs.iter().map(|run| run.text.as_str()).collect())
            .collect();
        lines.join("\n")
    }

    /// Adds `text` that looks like `look` to the last line, in the last run if that looks the same.
    pub(crate) fn push(&mut self, text: &str, look: Look) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        let Some(line) = self.lines.last_mut() else { return };
        match line.last_mut() {
            Some(run) if run.look == look => run.text.push_str(text),
            _ => line.push(StyledRun {
                text: text.to_string(),
                look,
            }),
        }
    }

    /// Drops the blanks at the end of the last line.
    pub(crate) fn trim_line(&mut self) {
        let Some(line) = self.lines.last_mut() else { return };
        while let Some(run) = line.last_mut() {
            run.text.truncate(run.text.trim_end().len());
            if !run.text.is_empty() {
                break;
            }
            line.pop();
        }
    }

    /// Starts a new line.
    pub(crate) fn new_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.lines.push(Vec::new());
    }
}

impl Session {
    /// The selected text with its colours and type styles; none without a selection.
    pub fn selected_styled(&self) -> Option<StyledText> {
        let palette = self.shared.palette.lock_ignoring_poison().clone();
        self.shared.own.lock().selected_styled(&palette)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn look(r: u8) -> Look {
        Look {
            fg: Rgb::new(r, 0, 0),
            bg: Rgb::default(),
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    #[test]
    fn runs_that_look_the_same_join_and_trailing_blanks_go() {
        let mut text = StyledText {
            lines: vec![],
            foreground: Rgb::default(),
            background: Rgb::default(),
        };
        text.push("ab", look(1));
        text.push("c", look(1));
        text.push("d ", look(2));
        text.push("   ", look(3));
        text.trim_line();
        text.new_line();
        text.push("x", look(1));
        assert_eq!(text.lines[0].len(), 2);
        assert_eq!(text.lines[0][0].text, "abc");
        assert_eq!(text.plain(), "abcd\nx");
    }
}
