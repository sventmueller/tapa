//! Vim keys in the input field (F95, setting `input.vim`) without AppKit, carried out by `term-vim` as in copy mode.
//! The field starts each command in Insert mode, where the text view types as always; Escape enters Normal mode on a
//! copy of the field's text, and the keys of Normal and Visual mode move, change and select on that copy until one
//! of them enters Insert mode again. Return sends from any mode, and `k` and `j` on the first and last line step
//! through the history as in zsh's vi mode.

use term_vim::{Effect, Mode, Vim};

/// A key as the field's Vim mode reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VimKey {
    Char(char),
    /// A letter with ⌃.
    Ctrl(char),
    Escape,
    Enter,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// What a key asks of the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The text view handles the key itself, as in Insert mode.
    Pass,
    /// The field shows `text` with the characters from `caret`, `selected` long, selected; `copy` goes on the
    /// pasteboard.
    Show {
        text: String,
        caret: usize,
        selected: usize,
        copy: Option<String>,
    },
    /// Send the command.
    Submit,
    /// Show the history's older or newer entry.
    History(bool),
}

/// The field's Vim state; Insert mode while there is no copy.
#[derive(Default)]
pub(crate) struct FieldVim {
    vim: Option<Vim>,
    /// The texts before each change, for `u`, and those undone, for ⌃R.
    undo: Vec<String>,
    redo: Vec<String>,
}

impl std::fmt::Debug for FieldVim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldVim")
            .field("mode", &self.mode())
            .finish_non_exhaustive()
    }
}

impl FieldVim {
    /// The mode the keys are in.
    pub(crate) fn mode(&self) -> Mode {
        self.vim.as_ref().map_or(Mode::Insert, Vim::mode)
    }

    /// Back to Insert mode for a new command line.
    pub(crate) fn reset(&mut self) {
        *self = FieldVim::default();
    }

    /// What the mode line shows, none in Insert mode.
    pub(crate) fn status(&self) -> Option<String> {
        use splat_i18n::t;
        let vim = self.vim.as_ref()?;
        if let Some(prompt) = vim.prompt() {
            return Some(prompt);
        }
        Some(match vim.mode() {
            Mode::Insert => return None,
            Mode::Normal => t("Vim: Normal").to_string(),
            Mode::Visual => t("Vim: Visuell").to_string(),
            Mode::VisualLine => t("Vim: Zeilen").to_string(),
            Mode::VisualBlock => t("Vim: Block").to_string(),
            Mode::Replace => t("Vim: Ersetzen").to_string(),
            Mode::Prompt => t("Vim: Suche").to_string(),
        })
    }

    /// Carries out `key` on `text` with the caret `caret` characters in.
    pub(crate) fn key(&mut self, key: VimKey, text: &str, caret: usize) -> Answer {
        match (self.vim.is_some(), key) {
            (_, VimKey::Enter) if self.mode() != Mode::Prompt => Answer::Submit,
            (false, VimKey::Escape) => self.enter_normal(text, caret),
            (false, _) => Answer::Pass,
            (true, _) => self.normal_key(key, text),
        }
    }

    fn enter_normal(&mut self, text: &str, caret: usize) -> Answer {
        let mut vim = Vim::new(text);
        let (line, column) = line_and_column(text, caret);
        vim.set_cursor(line, column.saturating_sub(1));
        self.vim = Some(vim);
        self.show(text, None)
    }

    fn normal_key(&mut self, key: VimKey, text: &str) -> Answer {
        let Some(vim) = self.vim.as_mut() else {
            return Answer::Pass;
        };
        let idle = vim.mode() == Mode::Normal;
        let (line, _) = vim.cursor();
        let lines = text.split('\n').count();
        match key {
            VimKey::Char('k') | VimKey::Up if idle && line == 0 => return Answer::History(true),
            VimKey::Char('j') | VimKey::Down if idle && line + 1 >= lines => return Answer::History(false),
            _ => {}
        }
        let name = match key {
            VimKey::Char(c) => {
                let outcome = vim.char(c);
                return self.after(outcome, text);
            }
            VimKey::Ctrl(c) => format!("<C-{}>", c.to_ascii_uppercase()),
            VimKey::Escape => "<Esc>".into(),
            VimKey::Enter => "<Enter>".into(),
            VimKey::Backspace => "<BS>".into(),
            VimKey::Up => "<Up>".into(),
            VimKey::Down => "<Down>".into(),
            VimKey::Left => "<Left>".into(),
            VimKey::Right => "<Right>".into(),
            VimKey::Home => "0".into(),
            VimKey::End => "$".into(),
        };
        let outcome = vim.keys(&name);
        self.after(outcome, text)
    }

    /// The field after `outcome` changed the copy of `before`: undo and redo go to the field's own history, and a key
    /// that entered Insert mode hands typing back to the text view.
    fn after(&mut self, outcome: term_vim::Outcome, before: &str) -> Answer {
        let mut copy = None;
        let mut restored = None;
        for effect in outcome.effects {
            match effect {
                Effect::Undo(steps) => {
                    for _ in 0..steps {
                        if let Some(text) = self.undo.pop() {
                            self.redo.push(restored.take().unwrap_or_else(|| before.to_string()));
                            restored = Some(text);
                        }
                    }
                }
                Effect::Redo(steps) => {
                    for _ in 0..steps {
                        if let Some(text) = self.redo.pop() {
                            self.undo.push(restored.take().unwrap_or_else(|| before.to_string()));
                            restored = Some(text);
                        }
                    }
                }
                Effect::Copy(text) => copy = Some(text),
                _ => {}
            }
        }
        if let Some(text) = restored
            && let Some(vim) = self.vim.as_mut()
        {
            vim.resync(&text);
            return self.show(&text, copy);
        }
        if !outcome.replacements.is_empty() {
            self.undo.push(before.to_string());
            self.redo.clear();
        }
        let text = self.vim.as_ref().map(Vim::text).unwrap_or_default();
        if self.mode() == Mode::Insert {
            let caret = self.vim.as_mut().map_or(0, |vim| {
                let (line, column) = vim.cursor();
                offset_of(&text, line, column)
            });
            self.vim = None;
            return Answer::Show {
                text,
                caret,
                selected: 0,
                copy,
            };
        }
        self.show(&text, copy)
    }

    /// The copy's text with the cursor as a one-character selection, or Visual mode's selection.
    fn show(&mut self, text: &str, copy: Option<String>) -> Answer {
        let Some(vim) = self.vim.as_mut() else {
            return Answer::Pass;
        };
        let (caret, selected) = match vim.selection() {
            Some(((top, from), (bottom, to))) => {
                let start = offset_of(text, top, from);
                let end = offset_of(text, bottom, to) + 1;
                (start, end.saturating_sub(start))
            }
            None => {
                let (line, column) = vim.cursor();
                let at = offset_of(text, line, column);
                (at, usize::from(at < text.chars().count()))
            }
        };
        Answer::Show {
            text: text.to_string(),
            caret,
            selected,
            copy,
        }
    }
}

/// The line and column in characters of the character `caret` characters into `text`.
fn line_and_column(text: &str, caret: usize) -> (usize, usize) {
    let before: String = text.chars().take(caret).collect();
    let line = before.matches('\n').count();
    let column = before.rsplit('\n').next().map_or(0, |last| last.chars().count());
    (line, column)
}

/// How many characters into `text` the place at `line` and `column` is, kept within the line.
fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let mut offset = 0;
    for (index, row) in text.split('\n').enumerate() {
        let length = row.chars().count();
        if index == line {
            return offset + column.min(length);
        }
        offset += length + 1;
    }
    text.chars().count()
}

/// The keys of `keys` written as Vim writes them: characters, and names such as `<Esc>`, `<CR>` and `<C-r>`.
#[cfg(any(test, feature = "selftest"))]
pub(crate) fn parse(keys: &str) -> Vec<VimKey> {
    let mut out = Vec::new();
    let mut rest = keys;
    while let Some(first) = rest.chars().next() {
        let named = (first == '<')
            .then(|| rest.find('>'))
            .flatten()
            .and_then(|end| Some((named_key(&rest[1..end])?, end + 1)));
        if let Some((key, length)) = named {
            out.push(key);
            rest = &rest[length..];
        } else {
            out.push(VimKey::Char(first));
            rest = &rest[first.len_utf8()..];
        }
    }
    out
}

#[cfg(any(test, feature = "selftest"))]
fn named_key(name: &str) -> Option<VimKey> {
    if let Some(letter) = name.strip_prefix("C-") {
        let mut chars = letter.chars();
        return match (chars.next(), chars.next()) {
            (Some(c), None) => Some(VimKey::Ctrl(c.to_ascii_lowercase())),
            _ => None,
        };
    }
    Some(match name {
        "Esc" => VimKey::Escape,
        "CR" | "Enter" => VimKey::Enter,
        "BS" => VimKey::Backspace,
        "Up" => VimKey::Up,
        "Down" => VimKey::Down,
        "Left" => VimKey::Left,
        "Right" => VimKey::Right,
        "Home" => VimKey::Home,
        "End" => VimKey::End,
        "lt" => VimKey::Char('<'),
        "Space" => VimKey::Char(' '),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `keys` to `vim` on `text` with the caret at its end, as the field would, typing in Insert mode.
    fn drive(vim: &mut FieldVim, text: &str, keys: &str) -> (String, Vec<Answer>) {
        let mut text = text.to_string();
        let mut caret = text.chars().count();
        let mut answers = Vec::new();
        for key in parse(keys) {
            let answer = vim.key(key, &text, caret);
            match (&answer, key) {
                (
                    Answer::Show {
                        text: shown, caret: at, ..
                    },
                    _,
                ) => {
                    text.clone_from(shown);
                    caret = *at;
                }
                (Answer::Pass, VimKey::Char(c)) => {
                    let mut chars: Vec<char> = text.chars().collect();
                    chars.insert(caret, c);
                    text = chars.into_iter().collect();
                    caret += 1;
                }
                _ => {}
            }
            answers.push(answer);
        }
        (text, answers)
    }

    #[test]
    fn escape_enters_normal_mode_where_keys_edit_and_i_types_again() {
        let mut vim = FieldVim::default();
        let (text, _) = drive(&mut vim, "git commit --amend", "<Esc>bdw");
        assert_eq!(text, "git commit --");
        assert_eq!(vim.mode(), Mode::Normal);
        let (text, _) = drive(&mut vim, &text, "0cwjj<Esc>");
        assert_eq!(text, "jj commit --");
        let (text, _) = drive(&mut vim, &text, "u");
        assert_eq!(text, "git commit --", "u restores the text before the change");
        let (text, _) = drive(&mut vim, &text, "<C-r>");
        assert_eq!(text, "jj commit --");
        let (text, _) = drive(&mut vim, &text, "A x");
        assert_eq!(text, "jj commit -- x");
        assert_eq!(vim.mode(), Mode::Insert);
    }

    #[test]
    fn return_sends_from_every_mode_and_k_steps_into_the_history() {
        let mut vim = FieldVim::default();
        assert_eq!(vim.key(VimKey::Enter, "ls", 2), Answer::Submit);
        assert_eq!(
            vim.key(VimKey::Char('k'), "ls", 2),
            Answer::Pass,
            "k types in Insert mode"
        );
        drive(&mut vim, "ls", "<Esc>");
        assert_eq!(vim.key(VimKey::Char('k'), "ls", 1), Answer::History(true));
        assert_eq!(vim.key(VimKey::Enter, "ls", 1), Answer::Submit);
        assert!(vim.status().is_some());
        vim.reset();
        assert_eq!(vim.status(), None);
    }

    #[test]
    fn the_cursor_shows_as_a_selected_character_and_visual_mode_as_its_selection() {
        let mut vim = FieldVim::default();
        let (_, answers) = drive(&mut vim, "echo hi", "<Esc>0");
        assert!(matches!(
            answers.last(),
            Some(Answer::Show {
                caret: 0,
                selected: 1,
                ..
            })
        ));
        let (_, answers) = drive(&mut vim, "echo hi", "ve");
        assert!(matches!(
            answers.last(),
            Some(Answer::Show {
                caret: 0,
                selected: 4,
                ..
            })
        ));
        assert_eq!(line_and_column("a\nbc", 3), (1, 1));
        assert_eq!(offset_of("a\nbc", 1, 1), 3);
    }
}
