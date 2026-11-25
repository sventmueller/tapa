//! The input field's state without AppKit (E7): when it shows, what Return sends, the history it steps through.
//! It shows only between OSC 133 B and C, as the shell waits for a command; never in a full-screen program; and not
//! after the shell asked for more of a command line it took (PS2, heredoc), until the next prompt.

use std::time::Duration;

/// Where the shell is, from its marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Phase {
    /// No mark came yet: the shell has no integration, and the field stays away.
    #[default]
    Unknown,
    /// The prompt is being drawn (A).
    Prompt,
    /// The shell waits for input (B).
    Input,
    /// A command runs (C).
    Running,
}

/// How long after Return the shell has to start the command before the field takes it for a continuation.
pub(crate) const CONTINUATION_WAIT: Duration = Duration::from_millis(400);

/// The most entries the field's own history keeps.
const MOST_HISTORY: usize = 10_000;

/// The field of one session.
#[derive(Debug, Default)]
pub(crate) struct Field {
    /// The user wants the field in this session.
    pub(crate) enabled: bool,
    pub(crate) phase: Phase,
    /// The program shows the alternate screen.
    pub(crate) full_screen: bool,
    /// The shell asked for more after Return, or the user chose raw typing; until the next prompt.
    pub(crate) raw: bool,
    /// Return was sent and the shell has not started the command yet.
    pub(crate) submitted: bool,
    history: Vec<String>,
    /// Where ↑ and ↓ are in the history, counted from the newest; none while editing a draft.
    cursor: Option<usize>,
    /// What the user typed before stepping into the history.
    draft: String,
    /// Vim's mode and copy while the setting asks for Vim keys (F95).
    pub(crate) vim: super::vim::FieldVim,
}

impl Field {
    /// A field that is on or off for a new session, as the settings say.
    pub(crate) fn new(enabled: bool) -> Field {
        Field {
            enabled,
            ..Field::default()
        }
    }

    /// Whether the field shows now.
    pub(crate) fn visible(&self) -> bool {
        self.enabled && self.phase == Phase::Input && !self.full_screen && !self.raw
    }

    pub(crate) fn prompt(&mut self) {
        self.phase = Phase::Prompt;
        self.raw = false;
        self.submitted = false;
    }

    pub(crate) fn input(&mut self) {
        self.phase = Phase::Input;
    }

    pub(crate) fn command_started(&mut self) {
        self.phase = Phase::Running;
        self.submitted = false;
    }

    /// The bytes Return sends for `text`: one bracketed paste where the shell asked for it, then CR. The text goes
    /// into the history.
    pub(crate) fn submit(&mut self, text: &str, bracketed: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(text.len() + 13);
        if bracketed && !text.is_empty() {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.replace('\n', "\r").as_bytes());
        }
        bytes.push(b'\r');
        let line = text.trim();
        if !line.is_empty() && self.history.last().map(String::as_str) != Some(line) {
            if self.history.len() >= MOST_HISTORY {
                self.history.remove(0);
            }
            self.history.push(line.to_string());
        }
        self.cursor = None;
        self.draft.clear();
        self.vim.reset();
        self.submitted = true;
        bytes
    }

    /// Return was sent but no command started in time: the shell wants more, which the user types raw.
    pub(crate) fn continuation_timed_out(&mut self) {
        if self.submitted && self.phase == Phase::Input {
            self.raw = true;
        }
        self.submitted = false;
    }

    /// The entry ↑ shows, older than the one shown; `current` is the field's text, kept as the draft.
    pub(crate) fn older(&mut self, current: &str) -> Option<String> {
        let next = match self.cursor {
            None => {
                self.draft = current.to_string();
                0
            }
            Some(at) => at + 1,
        };
        let entry = self.history.iter().rev().nth(next)?.clone();
        self.cursor = Some(next);
        Some(entry)
    }

    /// The entry ↓ shows, newer than the one shown; past the newest, the draft.
    pub(crate) fn newer(&mut self) -> Option<String> {
        match self.cursor? {
            0 => {
                self.cursor = None;
                Some(std::mem::take(&mut self.draft))
            }
            at => {
                self.cursor = Some(at - 1);
                self.history.iter().rev().nth(at - 1).cloned()
            }
        }
    }

    /// The newest entry older than the one shown that holds `needle`, as ⌃R finds it.
    pub(crate) fn search(&mut self, needle: &str) -> Option<String> {
        let start = self.cursor.map_or(0, |at| at + 1);
        let (index, entry) = self
            .history
            .iter()
            .rev()
            .enumerate()
            .skip(start)
            .find(|(_, entry)| entry.contains(needle))?;
        self.cursor = Some(index);
        Some(entry.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_shows_only_while_the_shell_waits_for_input() {
        let mut field = Field {
            enabled: true,
            ..Field::default()
        };
        assert!(!field.visible(), "no integration, no field");
        field.prompt();
        assert!(!field.visible());
        field.input();
        assert!(field.visible());
        field.full_screen = true;
        assert!(!field.visible());
        field.full_screen = false;
        field.command_started();
        assert!(!field.visible());
    }

    #[test]
    fn return_sends_one_bracketed_paste_and_a_continuation_turns_raw_until_the_prompt() {
        let mut field = Field {
            enabled: true,
            ..Field::default()
        };
        field.prompt();
        field.input();
        assert_eq!(
            field.submit("echo a\necho b", true),
            b"\x1b[200~echo a\necho b\x1b[201~\r"
        );
        assert_eq!(field.submit("ls", false), b"ls\r");
        field.continuation_timed_out();
        assert!(!field.visible(), "the shell asked for more");
        field.prompt();
        field.input();
        assert!(field.visible());
    }

    #[test]
    fn history_steps_back_and_forth_and_keeps_the_draft() {
        let mut field = Field::default();
        field.submit("git status", true);
        field.submit("cargo test", true);
        field.submit("git log", true);
        assert_eq!(field.older("gi").as_deref(), Some("git log"));
        assert_eq!(field.older("").as_deref(), Some("cargo test"));
        assert_eq!(field.newer().as_deref(), Some("git log"));
        assert_eq!(field.newer().as_deref(), Some("gi"));
        assert_eq!(field.newer(), None);
        assert_eq!(field.search("git").as_deref(), Some("git log"));
        assert_eq!(field.search("git").as_deref(), Some("git status"));
        assert_eq!(field.search("git"), None);
    }
}
