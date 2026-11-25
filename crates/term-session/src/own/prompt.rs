//! The live prompt left out of the view while the input field stands in for it (E7): the lines from the newest
//! prompt's mark to the cursor, once the shell waits for input there. The history keeps them; they show again as soon
//! as the command starts or the field steps aside.

use term_vt::term::{Block, BlockState, Modes};

use super::Own;

/// How the view leaves out the live prompt, as global lines, both ends inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LivePrompt {
    /// The prompt ends on the screen's last line: the lines above move down into its place.
    Pull(u64, u64),
    /// Blank lines follow below the prompt: they move up, and the lines above stay where they are.
    Drop(u64, u64),
}

/// The live prompt of `blocks`, oldest first, with the cursor on global line `cursor` and `last` the screen's last
/// line; none unless the newest block waits for input.
pub(crate) fn live_prompt(blocks: &[Block], cursor: u64, last: u64) -> Option<LivePrompt> {
    let block = blocks.last()?;
    if block.state != BlockState::Prompt || block.input.is_none() || block.prompt > cursor {
        return None;
    }
    Some(if cursor >= last {
        LivePrompt::Pull(block.prompt, cursor)
    } else {
        LivePrompt::Drop(block.prompt, cursor)
    })
}

impl Own {
    /// The live prompt the view leaves out now; none while the field does not ask for it or a full-screen program
    /// shows.
    pub(crate) fn live_prompt(&self) -> Option<LivePrompt> {
        if !self.hide_prompt || self.terminal.modes().contains(Modes::ALT_SCREEN) {
            return None;
        }
        let grid = self.terminal.grid();
        let last = grid.global(grid.history.len() + grid.lines() - 1);
        live_prompt(&self.block_list(), self.terminal.main_cursor_line(), last)
    }
}

#[cfg(test)]
mod tests {
    use term_vt::Terminal;
    use term_vt::term::Size;

    use super::super::Own;
    use super::{LivePrompt, live_prompt};
    use crate::session::Scroll;
    use crate::snapshot::Palette;

    fn rows(own: &Own) -> Vec<String> {
        own.snapshot(&Palette::default())
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn command(terminal: &mut Terminal, name: &str, output: usize) {
        terminal.advance(format!("\x1b]133;A\x07$ \x1b]133;B\x07{name}\r\n\x1b]133;C\x07").as_bytes());
        for line in 0..output {
            terminal.advance(format!("{name}.{line}\r\n").as_bytes());
        }
        terminal.advance(b"\x1b]133;D;0\x07");
    }

    fn own(lines: usize, outputs: &[usize]) -> Own {
        let mut terminal = Terminal::new(Size { columns: 20, lines }, 100);
        for (n, &output) in outputs.iter().enumerate() {
            command(&mut terminal, &format!("c{n}"), output);
        }
        terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        let mut own = Own::new(terminal);
        own.hide_prompt = true;
        own
    }

    #[test]
    fn only_a_prompt_that_waits_for_input_is_left_out() {
        let mut terminal = Terminal::new(Size { columns: 20, lines: 4 }, 100);
        assert_eq!(live_prompt(&terminal.blocks(), 0, 3), None, "no marks");
        terminal.advance(b"\x1b]133;A\x07$ ");
        assert_eq!(live_prompt(&terminal.blocks(), 0, 3), None, "the prompt is still drawn");
        terminal.advance(b"\x1b]133;B\x07");
        assert_eq!(live_prompt(&terminal.blocks(), 0, 3), Some(LivePrompt::Drop(0, 0)));
        assert_eq!(live_prompt(&terminal.blocks(), 3, 3), Some(LivePrompt::Pull(0, 3)));
        terminal.advance(b"ls\r\n\x1b]133;C\x07");
        assert_eq!(live_prompt(&terminal.blocks(), 1, 3), None, "the command runs");
    }

    #[test]
    fn at_the_bottom_the_lines_above_move_down_into_the_prompts_place() {
        let mut own = own(4, &[2, 2]);
        assert_eq!(rows(&own), ["c0.1", "$ c1", "c1.0", "c1.1"]);
        assert!(
            !own.snapshot(&Palette::default()).cursor.visible,
            "the cursor is in the field"
        );
        own.scroll(Scroll::Lines(1));
        assert_eq!(rows(&own), ["c0.0", "c0.1", "$ c1", "c1.0"]);
        own.scroll(Scroll::Lines(-1));
        assert_eq!(rows(&own), ["c0.1", "$ c1", "c1.0", "c1.1"]);
        assert_eq!(own.terminal.grid().display_offset, 0);
        own.hide_prompt = false;
        assert_eq!(rows(&own), ["$ c1", "c1.0", "c1.1", "$"]);
    }

    #[test]
    fn above_blank_lines_the_prompt_goes_and_the_rest_stays() {
        let mut own = own(4, &[0]);
        assert_eq!(rows(&own), ["$ c0", "", "", ""]);
        own.terminal.advance(b"ls\r\n\x1b]133;C\x07");
        assert_eq!(
            rows(&own),
            ["$ c0", "$ ls", "", ""],
            "the command and its prompt go into the history"
        );
    }

    #[test]
    fn a_full_screen_program_shows_its_own_screen() {
        let mut own = own(4, &[2, 2]);
        own.terminal.advance(b"\x1b[?1049h\x1b[Hfull");
        assert_eq!(rows(&own), ["full", "", "", ""]);
    }
}
