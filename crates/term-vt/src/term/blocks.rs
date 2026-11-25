//! Command blocks (F78, F79): each prompt with the command typed at it and the command's output, worked out from the
//! shell integration marks; and what a click into the line being typed at a prompt means for its cursor (F83).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::Terminal;
use super::marks::MarkKind;
use crate::cell::CellFlags;
use crate::scan::WorkingDirectory;
use crate::selection::{Anchor, Kind, Selection, Side};

/// How far a block has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// The shell shows the prompt and waits for a command.
    Prompt,
    /// The command runs.
    Running,
    /// The command ended.
    Finished,
}

/// A prompt, the command entered at it and the command's output, in global line numbers of the main screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The number of its prompt's mark, which names the block while the history drops older ones.
    pub id: u64,
    /// The line of the prompt.
    pub prompt: u64,
    /// Where the user's input begins.
    pub input: Option<(u64, usize)>,
    /// The line the output begins on.
    pub output: Option<u64>,
    /// Where the command ended.
    pub finish: Option<(u64, usize)>,
    /// The last line, before the next prompt; none for the newest block, which runs to the cursor.
    pub end: Option<u64>,
    /// The command line.
    pub command: Option<String>,
    /// The exit code, once the command ended and the shell reported one.
    pub exit_code: Option<i32>,
    /// How far it has come.
    pub state: BlockState,
    /// When the command started.
    pub started: Option<SystemTime>,
    /// When it ended.
    pub finished: Option<SystemTime>,
    /// The working directory at the prompt.
    pub directory: Option<Arc<WorkingDirectory>>,
}

impl Block {
    fn at_prompt(id: u64, prompt: u64, directory: Option<Arc<WorkingDirectory>>) -> Block {
        Block {
            id,
            prompt,
            input: None,
            output: None,
            finish: None,
            end: None,
            command: None,
            exit_code: None,
            state: BlockState::Prompt,
            started: None,
            finished: None,
            directory,
        }
    }

    /// The last line, with `newest` the line the newest block runs to.
    pub fn last_line(&self, newest: u64) -> u64 {
        self.end.unwrap_or(newest).max(self.prompt)
    }

    /// The first and last line of the output, none while there is none; `newest` as for `last_line`.
    pub fn output_lines(&self, newest: u64) -> Option<(u64, u64)> {
        let first = self.output?;
        let last = match self.finish {
            // The shell marks the end where the cursor stands after the output, at the start of the next line.
            Some((line, 0)) => line.checked_sub(1)?,
            Some((line, _)) => line,
            None => newest,
        }
        .min(self.last_line(newest));
        (first <= last).then_some((first, last))
    }

    /// The command ended with an exit code other than 0.
    pub fn failed(&self) -> bool {
        self.exit_code.is_some_and(|code| code != 0)
    }

    /// How long the command ran, or has run by `now`.
    pub fn duration(&self, now: SystemTime) -> Option<Duration> {
        let started = self.started?;
        self.finished.unwrap_or(now).duration_since(started).ok()
    }
}

impl Terminal {
    /// The blocks of the main screen and its history, oldest first.
    pub fn blocks(&self) -> Vec<Block> {
        let mut blocks: Vec<Block> = Vec::new();
        for (line, mark) in self.marks() {
            if mark.kind == MarkKind::Prompt {
                if let Some(last) = blocks.last_mut() {
                    // A prompt drawn again on the same line, as after an empty command line, stays one block.
                    if last.prompt == line && last.state == BlockState::Prompt {
                        last.directory.clone_from(&mark.directory);
                        continue;
                    }
                    last.end = Some(line.saturating_sub(1).max(last.prompt));
                }
                blocks.push(Block::at_prompt(mark.number, line, mark.directory.clone()));
                continue;
            }
            if mark.kind == MarkKind::Bookmark {
                continue;
            }
            if blocks.last().is_none_or(|last| last.end.is_some()) {
                blocks.push(Block::at_prompt(mark.number, line, mark.directory.clone()));
            }
            let Some(block) = blocks.last_mut() else { continue };
            match &mark.kind {
                MarkKind::Input => block.input = Some((line, mark.column)),
                MarkKind::Output(command) => {
                    block.output = Some(line);
                    block.command.clone_from(command);
                    block.started = Some(mark.at);
                    block.state = BlockState::Running;
                }
                MarkKind::Finished(code) if block.state == BlockState::Running => {
                    block.finish = Some((line, mark.column));
                    block.exit_code = *code;
                    block.finished = Some(mark.at);
                    block.state = BlockState::Finished;
                }
                _ => {}
            }
        }
        for block in &mut blocks {
            if block.command.is_none()
                && let (Some(input), Some(output)) = (block.input, block.output)
            {
                block.command = self.typed_text(input, output).filter(|text| !text.is_empty());
            }
        }
        blocks
    }

    /// The global line of the main screen's cursor, where the newest block ends.
    pub fn main_cursor_line(&self) -> u64 {
        let (grid, cursor) = if self.modes.contains(super::Modes::ALT_SCREEN) {
            (&self.inactive, &self.inactive_cursor)
        } else {
            (&self.grid, &self.cursor)
        };
        grid.global(grid.history.len() + cursor.line)
    }

    /// The text of the main screen's lines `first` to `last`: lines that wrapped join, others end in a newline.
    pub fn lines_text(&self, first: u64, last: u64) -> Option<String> {
        let grid = self.main_grid();
        let columns = grid.columns();
        let selection = Selection {
            kind: Kind::Line,
            start: (Anchor { line: first, column: 0 }, Side::Left),
            end: (
                Anchor {
                    line: last,
                    column: columns.saturating_sub(1),
                },
                Side::Right,
            ),
        };
        let text = selection.text(grid)?;
        Some(text.trim_end_matches('\n').to_string())
    }

    /// What the user typed from `input` to the line before `output`.
    fn typed_text(&self, input: (u64, usize), output: u64) -> Option<String> {
        let grid = self.main_grid();
        let mut text = String::new();
        for line in input.0..output.max(input.0 + 1) {
            let row = grid.row_global(line)?;
            let from = if line == input.0 { input.1 } else { 0 };
            for cell in row.cells.get(from..)? {
                if !cell.flags.contains(CellFlags::WIDE_SPACER) {
                    text.push(cell.c);
                }
            }
        }
        Some(text.trim().to_string())
    }

    /// How many characters the shell's cursor has to move, right when positive, to reach the cell at global line
    /// `line` and `column` of the line being typed at the prompt (F83); none unless the shell waits at a prompt for
    /// input and the cell is on that line.
    pub fn prompt_cursor_steps(&self, line: u64, column: usize) -> Option<isize> {
        if self.modes.contains(super::Modes::ALT_SCREEN) {
            return None;
        }
        let (_, input) = self
            .marks()
            .into_iter()
            .rev()
            .find(|(_, mark)| mark.kind != MarkKind::Bookmark)
            .filter(|(_, mark)| mark.kind == MarkKind::Input)
            .map(|(line, mark)| (line, (line, mark.column)))?;
        let grid = &self.grid;
        let cursor = (self.main_cursor_line(), self.cursor.column);
        let mut last = input.0.max(cursor.0);
        while grid.row_global(last).is_some_and(|row| row.wrapped()) && grid.row_global(last + 1).is_some() {
            last += 1;
        }
        if line < input.0 || line > last || cursor.0 < input.0 {
            return None;
        }
        let target = (line, column).max(input);
        let (mut before_target, mut before_cursor, mut written) = (0_usize, 0_usize, 0_usize);
        let mut count = 0_usize;
        for at in input.0..=last {
            let row = grid.row_global(at)?;
            let from = if at == input.0 { input.1 } else { 0 };
            for (index, cell) in row.cells.iter().enumerate().skip(from) {
                if cell
                    .flags
                    .intersects(CellFlags(CellFlags::WIDE_SPACER.0 | CellFlags::LEADING_WIDE_SPACER.0))
                {
                    continue;
                }
                if (at, index) < target {
                    before_target += 1;
                }
                if (at, index) < cursor {
                    before_cursor += 1;
                }
                count += 1;
                if cell.c != ' ' {
                    written = count;
                }
            }
        }
        let target = before_target.min(written.max(before_cursor));
        let steps = isize::try_from(target).ok()? - isize::try_from(before_cursor).ok()?;
        Some(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::BlockState;
    use crate::term::{Size, Terminal};

    fn terminal() -> Terminal {
        Terminal::new(Size { columns: 20, lines: 6 }, 100)
    }

    #[test]
    fn each_command_with_its_output_is_a_block() {
        let mut t = terminal();
        t.advance(b"\x1b]7;file:///Users/u/src\x07\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n");
        t.advance(b"\x1b]133;C;cmdline_url=ls\x07a\r\nb\r\n\x1b]133;D;0\x07");
        t.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07false\r\n\x1b]133;C\x07\x1b]133;D;1\x07");
        t.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        let blocks = t.blocks();
        assert_eq!(blocks.len(), 3);
        let ls = &blocks[0];
        assert_eq!((ls.prompt, ls.output, ls.end), (0, Some(1), Some(2)));
        assert_eq!(ls.output_lines(t.main_cursor_line()), Some((1, 2)));
        assert_eq!(ls.command.as_deref(), Some("ls"));
        assert_eq!(ls.exit_code, Some(0));
        assert!(!ls.failed());
        assert_eq!(
            ls.directory.as_ref().map(|d| d.path.display().to_string()),
            Some("/Users/u/src".into())
        );
        assert!(ls.duration(std::time::SystemTime::now()).is_some());
        assert_eq!(t.lines_text(1, 2).as_deref(), Some("a\nb"));
        let failed = &blocks[1];
        assert_eq!(
            failed.command.as_deref(),
            Some("false"),
            "read from the cells without cmdline_url"
        );
        assert!(failed.failed());
        assert_eq!(failed.output_lines(t.main_cursor_line()), None, "no output");
        assert_eq!(blocks[2].state, BlockState::Prompt);
        assert_eq!(blocks[2].end, None);
    }

    #[test]
    fn a_running_command_runs_to_the_cursor() {
        let mut t = terminal();
        t.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07make\r\n\x1b]133;C\x07one\r\ntwo");
        let blocks = t.blocks();
        assert_eq!(blocks[0].state, BlockState::Running);
        assert_eq!(blocks[0].output_lines(t.main_cursor_line()), Some((1, 2)));
        assert_eq!(blocks[0].exit_code, None);
    }

    #[test]
    fn a_prompt_drawn_again_on_its_line_stays_one_block() {
        let mut t = terminal();
        t.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07\r\x1b[K\x1b]133;A\x07$ \x1b]133;B\x07");
        assert_eq!(t.blocks().len(), 1);
    }

    #[test]
    fn bookmarks_toggle_and_are_jumped_to() {
        let mut t = Terminal::new(Size { columns: 20, lines: 3 }, 100);
        for n in 0..10 {
            t.advance(format!("line {n}\r\n").as_bytes());
        }
        let version = t.marks_version();
        assert!(t.toggle_bookmark(2));
        assert!(t.toggle_bookmark(5));
        assert_eq!(t.bookmark_lines(), [2, 5]);
        assert!(!t.toggle_bookmark(2));
        assert_eq!(t.bookmark_lines(), [5]);
        assert!(t.marks_version() > version);
        assert!(t.jump_to_bookmark(-1));
        let top = t.grid().view_row(0).unwrap().text();
        assert_eq!(top.trim_end(), "line 5");
        assert!(!t.toggle_bookmark(1000), "no such line");
        assert!(t.blocks().is_empty(), "bookmarks are no blocks");
    }

    #[test]
    fn a_click_in_the_typed_line_moves_the_cursor_by_characters() {
        let mut t = terminal();
        t.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07git status");
        let line = t.main_cursor_line();
        assert_eq!(t.prompt_cursor_steps(line, 2), Some(-10), "the start of the input");
        assert_eq!(t.prompt_cursor_steps(line, 0), Some(-10), "the prompt is no input");
        assert_eq!(t.prompt_cursor_steps(line, 6), Some(-6));
        assert_eq!(
            t.prompt_cursor_steps(line, 19),
            Some(0),
            "past the end stays at the end"
        );
        assert_eq!(t.prompt_cursor_steps(line + 1, 0), None);
        t.advance("\x1b[4G".as_bytes());
        assert_eq!(t.prompt_cursor_steps(line, 12), Some(9));
        t.advance(b"\r\n\x1b]133;C\x07");
        assert_eq!(t.prompt_cursor_steps(line, 4), None, "no prompt while a command runs");
    }

    #[test]
    fn wide_characters_are_one_step() {
        let mut t = terminal();
        t.advance("\x1b]133;A\x07$ \x1b]133;B\x07日本x".as_bytes());
        let line = t.main_cursor_line();
        assert_eq!(t.prompt_cursor_steps(line, 4), Some(-2));
        assert_eq!(t.prompt_cursor_steps(line, 2), Some(-3));
    }
}
