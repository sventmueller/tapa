//! Shell integration marks (OSC 133) anchored to their rows (E4): where each prompt, input and command output began
//! and how the command ended, so that blocks, prompt navigation and the input field find them after the history
//! scrolled and the lines were rewrapped.

use std::sync::Arc;
use std::time::SystemTime;

use super::Terminal;
use crate::scan::WorkingDirectory;

/// What a mark marks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkKind {
    /// A prompt begins (A).
    Prompt,
    /// The prompt ends and the user's input begins (B).
    Input,
    /// The command line was entered and its output begins (C), with the line where the integration sent it.
    Output(Option<String>),
    /// The command ended (D) with its exit code.
    Finished(Option<i32>),
    /// The user marked the row to come back to it (F87).
    Bookmark,
}

/// A mark at a row and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// What it marks.
    pub kind: MarkKind,
    /// The serial of the row it is on.
    pub serial: u64,
    /// The column.
    pub column: usize,
    /// Counts the marks of a terminal from 0, so that it names the mark while older ones go.
    pub number: u64,
    /// When it was set.
    pub at: SystemTime,
    /// The working directory the shell had reported by then.
    pub directory: Option<Arc<WorkingDirectory>>,
}

impl Mark {
    /// A mark of `kind` at `column` of the row with `serial`, set now, numbered 0 and without a directory.
    pub fn new(kind: MarkKind, serial: u64, column: usize) -> Mark {
        Mark {
            kind,
            serial,
            column,
            number: 0,
            at: SystemTime::now(),
            directory: None,
        }
    }
}

/// The most marks a terminal keeps; older ones belong to rows the history dropped long ago.
const MOST_MARKS: usize = 20_000;

impl Terminal {
    /// Records the OSC 133 mark `body`, such as `A`, `C;cmdline_url=ls` or `D;0`, at the cursor.
    pub(super) fn mark(&mut self, parts: &[&[u8]]) {
        let Some(kind) = parts.first().and_then(|kind| kind.first()) else {
            return;
        };
        let option = |name: &str| {
            parts[1..]
                .iter()
                .filter_map(|part| std::str::from_utf8(part).ok())
                .find_map(|part| part.strip_prefix(name).map(str::to_string))
        };
        let kind = match kind {
            b'A' => MarkKind::Prompt,
            b'B' => MarkKind::Input,
            b'C' => MarkKind::Output(option("cmdline_url=").map(|url| percent_decode(&url))),
            b'D' => MarkKind::Finished(
                parts
                    .get(1)
                    .and_then(|code| std::str::from_utf8(code).ok())
                    .and_then(|code| code.parse().ok()),
            ),
            _ => return,
        };
        let serial = self.grid.rows[self.cursor.line].serial;
        self.push_mark(kind, serial, self.cursor.column);
    }

    fn push_mark(&mut self, kind: MarkKind, serial: u64, column: usize) {
        if self.marks.len() >= MOST_MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(Mark {
            kind,
            serial,
            column,
            number: self.next_mark,
            at: SystemTime::now(),
            directory: self.directory.clone(),
        });
        self.next_mark += 1;
    }

    /// Takes the working directory of OSC 7 `url` for the marks that follow.
    pub(super) fn set_directory(&mut self, url: &[u8]) {
        if let Some(directory) = crate::scan::parse_osc7(url) {
            self.directory = Some(Arc::new(directory));
        }
    }

    /// Changes whenever a mark comes or goes, so that what is worked out from the marks can be kept until then.
    pub fn marks_version(&self) -> u64 {
        self.next_mark
    }

    /// Sets a bookmark on the main screen's row with global number `line`, or removes the one there (F87); whether
    /// the row has one now.
    pub fn toggle_bookmark(&mut self, line: u64) -> bool {
        let Some(serial) = self.main_grid().row_global(line).map(|row| row.serial) else {
            return false;
        };
        let before = self.marks.len();
        self.marks
            .retain(|mark| !(mark.kind == MarkKind::Bookmark && mark.serial == serial));
        if self.marks.len() != before {
            self.next_mark += 1;
            return false;
        }
        self.push_mark(MarkKind::Bookmark, serial, 0);
        true
    }

    /// The global line numbers of the bookmarks, top first.
    pub fn bookmark_lines(&self) -> Vec<u64> {
        let mut lines: Vec<u64> = self
            .marks()
            .into_iter()
            .filter(|(_, mark)| mark.kind == MarkKind::Bookmark)
            .map(|(line, _)| line)
            .collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// Scrolls the view so that the bookmark `step` bookmarks from the view's top is at its top, as
    /// `jump_to_prompt` does for prompts. Whether the view moved.
    pub fn jump_to_bookmark(&mut self, step: isize) -> bool {
        let lines = self.bookmark_lines();
        self.jump_among(&lines, step)
    }

    /// The main screen, which the marks belong to, also while the alternate one shows.
    pub(super) fn main_grid(&self) -> &crate::grid::Grid {
        if self.modes.contains(super::Modes::ALT_SCREEN) {
            &self.inactive
        } else {
            &self.grid
        }
    }

    /// The marks whose rows the main screen and its history still have, each with its global line number, oldest
    /// first.
    pub fn marks(&self) -> Vec<(u64, &Mark)> {
        let grid = self.main_grid();
        let lines: std::collections::HashMap<u64, u64> = grid
            .history
            .serials()
            .chain(grid.rows.iter().map(|row| row.serial))
            .enumerate()
            .map(|(index, serial)| (serial, grid.global(index)))
            .collect();
        self.marks
            .iter()
            .filter_map(|mark| lines.get(&mark.serial).map(|&line| (line, mark)))
            .collect()
    }

    /// The global line numbers of the prompts, oldest first.
    pub fn prompt_lines(&self) -> Vec<u64> {
        self.marks()
            .into_iter()
            .filter(|(_, mark)| mark.kind == MarkKind::Prompt)
            .map(|(line, _)| line)
            .collect()
    }

    /// Scrolls the view so that the prompt `step` prompts from the one at its top is at its top: negative steps go to
    /// older prompts, positive ones to newer, past the newest back to the live screen. Whether the view moved.
    pub fn jump_to_prompt(&mut self, step: isize) -> bool {
        let prompts = self.prompt_lines();
        self.jump_among(&prompts, step)
    }

    /// Scrolls the view so that the line of `prompts`, oldest first, `step` lines on from the one at its top is at
    /// its top; past the newest back to the live screen.
    fn jump_among(&mut self, prompts: &[u64], step: isize) -> bool {
        if step == 0 {
            return false;
        }
        let grid = &self.grid;
        let top = grid.global_of_view(0);
        let target = if step < 0 {
            prompts
                .iter()
                .rev()
                .filter(|&&line| line < top)
                .nth(step.unsigned_abs() - 1)
                .copied()
        } else {
            prompts
                .iter()
                .filter(|&&line| line > top)
                .nth(step.unsigned_abs().saturating_sub(1))
                .copied()
        };
        let before = grid.display_offset;
        let grid = &mut self.grid;
        match target {
            Some(line) => {
                let index = usize::try_from(line - grid.dropped).unwrap_or(0);
                grid.display_offset = grid.history.len().saturating_sub(index);
            }
            None if step > 0 => grid.display_offset = 0,
            None => {}
        }
        grid.display_offset != before
    }

    /// The line the user types at the newest prompt while the shell waits for it (E7): none in a full-screen program,
    /// while a command runs, or once the prompt's input mark left the screen.
    pub fn typed_line(&self) -> Option<TypedLine> {
        if self.modes.contains(super::Modes::ALT_SCREEN) {
            return None;
        }
        let mark = self.marks.iter().rev().find(|mark| mark.kind != MarkKind::Bookmark)?;
        if mark.kind != MarkKind::Input {
            return None;
        }
        let grid = &self.grid;
        let cursor = self.cursor.line.min(grid.rows.len().checked_sub(1)?);
        let first = (0..=cursor).rev().find(|&line| grid.rows[line].serial == mark.serial)?;
        let mut text = String::new();
        for line in first..=cursor {
            let cells = &grid.rows[line].cells;
            let from = if line == first { mark.column } else { 0 };
            let to = if line == cursor {
                self.cursor.column
            } else {
                cells.len()
            };
            for cell in cells.get(from.min(to)..to.min(cells.len()))? {
                if !cell.flags.contains(crate::cell::CellFlags::WIDE_SPACER) {
                    text.push(cell.c);
                }
            }
        }
        let after = grid.rows[cursor].cells.get(self.cursor.column..).unwrap_or_default();
        let room = after.iter().take_while(|cell| cell.c == ' ').count();
        Some(TypedLine {
            text,
            room,
            open: room == after.len(),
        })
    }
}

/// What the user typed at the prompt and the room after it, for a suggestion drawn from the cursor on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedLine {
    /// The cells from the prompt's input mark to the cursor.
    pub text: String,
    /// The blank cells from the cursor to the next written one on its line, or to the line's end.
    pub room: usize,
    /// Nothing is written after the cursor on its line, such as a right prompt.
    pub open: bool,
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use crate::term::{Size, Terminal};

    #[test]
    fn jumping_to_prompts_scrolls_the_view_to_them() {
        let mut terminal = Terminal::new(Size { columns: 20, lines: 3 }, 100);
        for n in 0..4 {
            terminal.advance(format!("\x1b]133;A\x07$ cmd{n}\r\nout\r\nout\r\n").as_bytes());
        }
        let top = |t: &Terminal| t.grid().view_row(0).unwrap().text().trim_end().to_string();
        assert!(terminal.jump_to_prompt(-1));
        assert_eq!(top(&terminal), "$ cmd3");
        assert!(terminal.jump_to_prompt(-2));
        assert_eq!(top(&terminal), "$ cmd1");
        assert!(terminal.jump_to_prompt(1));
        assert_eq!(top(&terminal), "$ cmd2");
        terminal.jump_to_prompt(5);
        assert_eq!(terminal.grid().display_offset, 0);
    }

    #[test]
    fn prompts_stay_on_their_lines_as_the_history_grows_and_rewraps() {
        let mut terminal = Terminal::new(Size { columns: 10, lines: 3 }, 100);
        terminal
            .advance(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C;cmdline_url=ls\x07a\r\nb\r\n\x1b]133;D;0\x07");
        terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07git st");
        let prompts = terminal.prompt_lines();
        assert_eq!(prompts.len(), 2);
        assert_eq!(
            terminal.grid().row_global(prompts[0]).unwrap().text().trim_end(),
            "$ ls"
        );
        assert_eq!(
            terminal.typed_line().map(|typed| (typed.text, typed.room, typed.open)),
            Some(("git st".into(), 2, true))
        );
        terminal.resize(Size { columns: 4, lines: 3 });
        let prompts = terminal.prompt_lines();
        assert_eq!(
            terminal.grid().row_global(prompts[1]).unwrap().text().trim_end(),
            "$ gi"
        );
        let output = terminal.marks().into_iter().find_map(|(_, mark)| match &mark.kind {
            super::MarkKind::Output(command) => command.clone(),
            _ => None,
        });
        assert_eq!(output.as_deref(), Some("ls"));
    }
}
