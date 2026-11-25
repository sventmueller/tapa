//! Command blocks (F78, F79, F81, F84), bookmarks (F87) and the click into the line typed at a prompt (F83), as the
//! view asks for them.

use std::sync::atomic::Ordering;

pub use term_vt::term::{Block, BlockState, TypedLine};

use super::Session;
use crate::snapshot::Point;

/// A block with some of its lines in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownBlock {
    /// The block.
    pub block: Block,
    /// The first and last view line it covers.
    pub lines: (usize, usize),
    /// The view line of its prompt, where that shows.
    pub prompt_line: Option<usize>,
    /// Its output is collapsed.
    pub folded: bool,
    /// How many lines of output it has; for a collapsed block, how many it hides.
    pub output_lines: u64,
    /// The text of the line the command was typed on, for a header that stands in for it.
    pub heading: String,
}

/// What the view shows of the blocks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlocksView {
    /// The blocks in the view, top first.
    pub blocks: Vec<ShownBlock>,
    /// The block whose prompt scrolled off the top while its output still shows (F84).
    pub sticky: Option<ShownBlock>,
    /// The view lines with a bookmark.
    pub bookmarks: Vec<usize>,
    /// The block the block actions act on.
    pub current: Option<u64>,
}

/// Which text of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockPart {
    /// The command's output.
    Output,
    /// The command line.
    Command,
    /// Prompt, command and output.
    Whole,
}

impl Session {
    fn with_own<R>(&self, f: impl FnOnce(&mut crate::own::Own) -> R) -> R {
        f(&mut self.shared.own.lock())
    }

    /// The blocks the view shows; none without shell integration.
    pub fn blocks_view(&self) -> BlocksView {
        self.with_own(|own| own.blocks_view())
    }

    /// The text `part` of block `id`.
    pub fn block_text(&self, id: u64, part: BlockPart) -> Option<String> {
        self.with_own(|own| own.block_text(id, part))
    }

    /// The block the block actions act on: the one the user chose, else the one at the top of a view scrolled back,
    /// else the newest command's.
    pub fn current_block(&self) -> Option<u64> {
        self.with_own(|own| own.current_block())
    }

    /// Makes block `id` the one the block actions act on.
    pub fn choose_block(&self, id: u64) {
        self.with_own(|own| own.choose_block(id));
    }

    /// Selects block `id`, prompt to output, scrolling its prompt into view; whether there is such a block.
    pub fn select_block(&self, id: u64) -> bool {
        self.changed(self.with_own(|own| own.select_block(id)))
    }

    /// Collapses the output of block `id` or shows it again; whether it is collapsed now. Only a finished command
    /// with output collapses.
    pub fn toggle_block(&self, id: u64) -> bool {
        let folded = self.with_own(|own| own.toggle_fold(id));
        self.changed(folded.is_some());
        folded.unwrap_or(false)
    }

    /// Sets a bookmark on the first selected line, the top line of a view scrolled back or the cursor's line, or
    /// removes the one there; whether there is one now.
    pub fn toggle_bookmark(&self) -> bool {
        let set = self.with_own(|own| own.toggle_bookmark_here());
        self.changed(true);
        set
    }

    /// Scrolls to the bookmark `step` bookmarks from the view's top, as `jump_to_prompt` does; whether the view moved.
    pub fn jump_to_bookmark(&self, step: isize) -> bool {
        self.changed(self.with_own(|own| own.jump_to_bookmark(step)))
    }

    /// Moves the shell's cursor to the cell at view point `point` with arrow keys, when the shell waits at a prompt
    /// and the point is on the line being typed (F83); whether it sent any.
    pub fn click_in_prompt(&self, point: Point) -> bool {
        let Some((steps, app_cursor)) = self.with_own(|own| {
            let steps = own.prompt_steps(point)?;
            Some((steps, own.input_modes().app_cursor))
        }) else {
            return false;
        };
        if steps == 0 {
            return false;
        }
        let key: &[u8] = match (steps < 0, app_cursor) {
            (true, false) => b"\x1b[D",
            (true, true) => b"\x1bOD",
            (false, false) => b"\x1b[C",
            (false, true) => b"\x1bOC",
        };
        self.write(key.repeat(steps.unsigned_abs()));
        true
    }

    /// The line the user types at the shell's prompt and the room after it, with the cursor where the next frame
    /// shows it, for a suggestion drawn there (E7).
    pub fn typed_line(&self) -> Option<(TypedLine, Point)> {
        self.with_own(|own| {
            let typed = own.terminal.typed_line()?;
            let cursor = own.terminal.cursor();
            let shown = own.shown_lines();
            let offset = own.terminal.grid().display_offset;
            let line = own.cursor_view_line(shown.as_deref(), cursor.line + offset);
            Some((
                typed,
                Point {
                    line,
                    column: cursor.column,
                },
            ))
        })
    }

    /// Leaves the live prompt out of the view while `hide`, as the input field stands in for it (E7); whether that
    /// changed.
    pub fn hide_live_prompt(&self, hide: bool) -> bool {
        self.changed(self.with_own(|own| std::mem::replace(&mut own.hide_prompt, hide) != hide))
    }

    /// Has the next frame drawn in full after `changed` the view; hands `changed` back.
    fn changed(&self, changed: bool) -> bool {
        if changed {
            self.shared.full_redraw.store(true, Ordering::Release);
        }
        changed
    }
}
