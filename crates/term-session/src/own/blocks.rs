//! The own core's command blocks as the session shows them (F78, F84, F87): worked out from the marks once per change
//! of the marks or the size, collapsed ones hidden from the view, the block the actions act on, and bookmarks.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use term_vt::selection::{Anchor, Kind, Selection, Side};
use term_vt::term::{Block, BlockState, Modes};

use super::Own;
use super::fold::Hidden;
use super::prompt::LivePrompt;
use crate::session::{BlockPart, BlocksView, Scroll, ShownBlock};
use crate::snapshot::{Highlight, Point};

/// The blocks of an own core and what the user did to them.
#[derive(Debug, Default)]
pub(crate) struct Blocks {
    /// The ids of the collapsed blocks.
    folded: BTreeSet<u64>,
    /// The block the user chose, and how many blocks there were then; a new block ends the choice.
    chosen: Option<(u64, usize)>,
    cache: RefCell<Option<Cached>>,
}

/// What the marks gave at one version of them and one size.
#[derive(Debug)]
struct Cached {
    key: (u64, usize, usize),
    blocks: Arc<Vec<Block>>,
    bookmarks: Arc<Vec<u64>>,
    hidden: Option<Arc<Hidden>>,
}

impl Own {
    fn cached<R>(&self, f: impl FnOnce(&Cached) -> R) -> R {
        let terminal = &self.terminal;
        let size = terminal.size();
        let key = (terminal.marks_version(), size.columns, size.lines);
        let mut cache = self.blocks.cache.borrow_mut();
        let cached = match cache.take() {
            Some(cached) if cached.key == key => cached,
            _ => Cached {
                key,
                blocks: Arc::new(terminal.blocks()),
                bookmarks: Arc::new(terminal.bookmark_lines()),
                hidden: None,
            },
        };
        let cached = cache.insert(cached);
        if cached.hidden.is_none() && !self.blocks.folded.is_empty() {
            let newest = terminal.main_cursor_line();
            let ranges = cached
                .blocks
                .iter()
                .filter(|block| self.blocks.folded.contains(&block.id))
                .filter_map(|block| block.output_lines(newest))
                .collect();
            cached.hidden = Some(Arc::new(Hidden::new(ranges)));
        }
        f(cached)
    }

    pub(super) fn block_list(&self) -> Arc<Vec<Block>> {
        self.cached(|cached| cached.blocks.clone())
    }

    fn block(&self, id: u64) -> Option<Block> {
        let blocks = self.block_list();
        let at = blocks.binary_search_by_key(&id, |block| block.id).ok()?;
        blocks.get(at).cloned()
    }

    /// The lines collapsed blocks and a live prompt at the screen's bottom hide, none while nothing is collapsed or
    /// the alternate screen shows.
    fn hidden(&self) -> Option<Arc<Hidden>> {
        if self.terminal.modes().contains(Modes::ALT_SCREEN) {
            return None;
        }
        let folded = if self.blocks.folded.is_empty() {
            None
        } else {
            self.cached(|cached| cached.hidden.clone())
                .filter(|hidden| !hidden.is_empty())
        };
        match self.live_prompt() {
            Some(LivePrompt::Pull(first, last)) => Some(Arc::new(match folded {
                Some(folded) => folded.with((first, last)),
                None => Hidden::new(vec![(first, last)]),
            })),
            _ => folded,
        }
    }

    /// The global lines the view shows, top first, while collapsed blocks or the live prompt hide some; none while
    /// every line shows and the view is the grid's.
    pub(crate) fn shown_lines(&self) -> Option<Vec<u64>> {
        let dropped = match self.live_prompt() {
            Some(LivePrompt::Drop(first, last)) => Some(first..=last),
            _ => None,
        };
        let hidden = self.hidden();
        if hidden.is_none() && dropped.is_none() {
            return None;
        }
        let grid = self.terminal.grid();
        let mut lines = match hidden {
            Some(hidden) => {
                let first = grid.dropped;
                let last = grid.global(grid.history.len() + grid.lines() - 1);
                hidden.view(grid.global_of_view(0), first, last, grid.lines())
            }
            None => (0..grid.lines()).map(|line| grid.global_of_view(line)).collect(),
        };
        if let Some(dropped) = dropped {
            lines.retain(|line| !dropped.contains(line));
        }
        Some(lines)
    }

    /// The global line view line `line` shows.
    pub(crate) fn global_of_view(&self, shown: Option<&[u64]>, line: usize) -> u64 {
        let grid = self.terminal.grid();
        let line = line.min(grid.lines() - 1);
        match shown {
            Some(shown) => shown
                .get(line)
                .or(shown.last())
                .copied()
                .unwrap_or_else(|| grid.global_of_view(line)),
            None => grid.global_of_view(line),
        }
    }

    /// The view line the cursor is on, `line` without collapsed blocks.
    pub(crate) fn cursor_view_line(&self, shown: Option<&[u64]>, line: usize) -> usize {
        let Some(shown) = shown else { return line };
        let grid = self.terminal.grid();
        let global = grid.global(grid.history.len() + self.terminal.cursor().line);
        shown.binary_search(&global).unwrap_or(grid.lines())
    }

    /// The selection as the view shows it while collapsed blocks hide lines.
    pub(crate) fn folded_highlight(&self, shown: &[u64]) -> Option<Highlight> {
        let selection = self.selection.as_ref()?;
        let grid = self.terminal.grid();
        let (first, last) = selection.range(grid)?;
        let start = shown.partition_point(|&line| line < first.line);
        let end = shown.partition_point(|&line| line <= last.line).checked_sub(1)?;
        if start >= shown.len() || end < start {
            return None;
        }
        let column = |at: usize, anchor: Anchor, otherwise: usize| {
            if shown[at] == anchor.line {
                anchor.column
            } else {
                otherwise
            }
        };
        Some(Highlight {
            start: Point {
                line: start,
                column: column(start, first, 0),
            },
            end: Point {
                line: end,
                column: column(end, last, grid.columns() - 1),
            },
            block: selection.kind == Kind::Block,
        })
    }

    /// Scrolls by shown lines while collapsed blocks hide some; whether it did, else the grid scrolls as it is.
    pub(crate) fn scroll_folded(&mut self, scroll: Scroll) -> bool {
        let Some(hidden) = self.hidden() else {
            return false;
        };
        let Some(shown) = self.shown_lines() else {
            return false;
        };
        let lines = isize::try_from(self.terminal.size().lines).unwrap_or(1);
        let steps = match scroll {
            Scroll::Lines(delta) => isize::try_from(delta).unwrap_or(0),
            Scroll::PageUp => lines,
            Scroll::PageDown => -lines,
            Scroll::Top | Scroll::Bottom => return false,
        };
        let grid = self.terminal.grid_mut();
        let first = grid.dropped;
        let last = grid.global(grid.history.len() + grid.lines() - 1);
        let top = hidden.step(shown.first().copied().unwrap_or(first), steps, first, last);
        let at_end = hidden.view(top, first, last, grid.lines()).last().copied() == hidden.shown_until(last, first);
        let index = usize::try_from(top - first).unwrap_or(usize::MAX);
        grid.display_offset = if at_end {
            0
        } else {
            grid.history.len().saturating_sub(index)
        };
        true
    }

    /// Shows the lines of the collapsed block that hides `line` again.
    pub(crate) fn unfold_line(&mut self, line: u64) {
        let Some(hidden) = self.hidden() else { return };
        if hidden.range_of(line).is_none() {
            return;
        }
        let newest = self.terminal.main_cursor_line();
        let blocks = self.block_list();
        let hiding = blocks.iter().find(|block| {
            block
                .output_lines(newest)
                .is_some_and(|(first, last)| (first..=last).contains(&line))
        });
        if let Some(block) = hiding {
            self.set_folded(block.id, false);
        }
    }

    fn set_folded(&mut self, id: u64, folded: bool) {
        if folded {
            self.blocks.folded.insert(id);
        } else {
            self.blocks.folded.remove(&id);
        }
        if let Some(cached) = self.blocks.cache.get_mut().as_mut() {
            cached.hidden = None;
        }
    }

    /// Collapses block `id` or shows it again; whether it is collapsed now, none for a block that cannot collapse.
    pub(crate) fn toggle_fold(&mut self, id: u64) -> Option<bool> {
        let block = self.block(id)?;
        if self.blocks.folded.contains(&id) {
            self.set_folded(id, false);
            return Some(false);
        }
        let newest = self.terminal.main_cursor_line();
        if block.state != BlockState::Finished || block.output_lines(newest).is_none() {
            return None;
        }
        self.set_folded(id, true);
        Some(true)
    }

    pub(crate) fn choose_block(&mut self, id: u64) {
        let count = self.block_list().len();
        self.blocks.chosen = Some((id, count));
    }

    pub(crate) fn current_block(&self) -> Option<u64> {
        let blocks = self.block_list();
        if let Some((id, count)) = self.blocks.chosen
            && count == blocks.len()
            && blocks.binary_search_by_key(&id, |block| block.id).is_ok()
        {
            return Some(id);
        }
        let grid = self.terminal.grid();
        if self.terminal.modes().contains(Modes::ALT_SCREEN) {
            return None;
        }
        if grid.display_offset > 0 {
            let shown = self.shown_lines();
            let top = self.global_of_view(shown.as_deref(), 0);
            let newest = self.terminal.main_cursor_line();
            let at = blocks.partition_point(|block| block.last_line(newest) < top);
            return blocks.get(at).map(|block| block.id);
        }
        blocks
            .iter()
            .rev()
            .find(|block| block.state != BlockState::Prompt)
            .map(|block| block.id)
    }

    pub(crate) fn block_text(&self, id: u64, part: BlockPart) -> Option<String> {
        let block = self.block(id)?;
        let newest = self.terminal.main_cursor_line();
        match part {
            BlockPart::Command => block.command,
            BlockPart::Output => {
                let (first, last) = block.output_lines(newest)?;
                self.terminal.lines_text(first, last)
            }
            BlockPart::Whole => self.terminal.lines_text(block.prompt, block.last_line(newest)),
        }
    }

    pub(crate) fn select_block(&mut self, id: u64) -> bool {
        let Some(block) = self.block(id) else {
            return false;
        };
        let newest = self.terminal.main_cursor_line();
        let columns = self.terminal.size().columns;
        self.selection = Some(Selection {
            kind: Kind::Line,
            start: (
                Anchor {
                    line: block.prompt,
                    column: 0,
                },
                Side::Left,
            ),
            end: (
                Anchor {
                    line: block.last_line(newest),
                    column: columns.saturating_sub(1),
                },
                Side::Right,
            ),
        });
        self.choose_block(id);
        let shown = self.shown_lines();
        let in_view = shown.as_deref().map_or_else(
            || self.terminal.grid().view_of_global(block.prompt).is_some(),
            |shown| shown.binary_search(&block.prompt).is_ok(),
        );
        if !in_view {
            self.scroll_to_top(block.prompt);
        }
        true
    }

    /// Scrolls the view so that global line `line` is at its top.
    fn scroll_to_top(&mut self, line: u64) {
        let grid = self.terminal.grid_mut();
        let index = usize::try_from(line.saturating_sub(grid.dropped)).unwrap_or(usize::MAX);
        grid.display_offset = grid.history.len().saturating_sub(index);
    }

    pub(crate) fn toggle_bookmark_here(&mut self) -> bool {
        let grid = self.terminal.grid();
        let line = match self.selection.as_ref().and_then(|selection| selection.range(grid)) {
            Some((first, _)) => first.line,
            None if grid.display_offset > 0 => {
                let shown = self.shown_lines();
                self.global_of_view(shown.as_deref(), 0)
            }
            None => self.terminal.main_cursor_line(),
        };
        self.terminal.toggle_bookmark(line)
    }

    pub(crate) fn jump_to_bookmark(&mut self, step: isize) -> bool {
        let moved = self.terminal.jump_to_bookmark(step);
        if moved {
            let top = self.terminal.grid().global_of_view(0);
            self.unfold_line(top);
        }
        moved
    }

    pub(crate) fn prompt_steps(&self, point: Point) -> Option<isize> {
        let shown = self.shown_lines();
        let line = self.global_of_view(shown.as_deref(), point.line);
        self.terminal.prompt_cursor_steps(line, point.column)
    }

    pub(crate) fn blocks_view(&self) -> BlocksView {
        let terminal = &self.terminal;
        if terminal.modes().contains(Modes::ALT_SCREEN) || terminal.marks_version() == 0 {
            return BlocksView::default();
        }
        let (blocks, bookmarks) = self.cached(|cached| (cached.blocks.clone(), cached.bookmarks.clone()));
        let grid = terminal.grid();
        let shown = self
            .shown_lines()
            .unwrap_or_else(|| (0..grid.lines()).map(|line| grid.global_of_view(line)).collect());
        let (Some(&top), Some(&bottom)) = (shown.first(), shown.last()) else {
            return BlocksView::default();
        };
        let newest = terminal.main_cursor_line();
        let start = blocks.partition_point(|block| block.last_line(newest) < top);
        let mut view = BlocksView {
            current: self.current_block(),
            bookmarks: bookmarks
                .iter()
                .filter_map(|line| shown.binary_search(line).ok())
                .collect(),
            ..BlocksView::default()
        };
        for block in blocks[start..].iter().take_while(|block| block.prompt <= bottom) {
            let first = shown.partition_point(|&line| line < block.prompt);
            let Some(last) = shown
                .partition_point(|&line| line <= block.last_line(newest))
                .checked_sub(1)
            else {
                continue;
            };
            let output_lines = block.output_lines(newest).map_or(0, |(first, last)| last - first + 1);
            let heading_line = block.input.map_or(block.prompt, |(line, _)| line);
            let shown_block = ShownBlock {
                block: block.clone(),
                lines: (first, last.max(first)),
                prompt_line: shown.binary_search(&block.prompt).ok(),
                folded: self.blocks.folded.contains(&block.id),
                output_lines,
                heading: grid
                    .row_global(heading_line)
                    .map(|row| row.text().trim_end().to_string())
                    .unwrap_or_default(),
            };
            if block.prompt < top && block.state != BlockState::Prompt && view.sticky.is_none() {
                view.sticky = Some(shown_block.clone());
            }
            view.blocks.push(shown_block);
        }
        view
    }
}

#[cfg(test)]
mod tests {
    use term_vt::Terminal;
    use term_vt::term::Size;

    use super::super::Own;
    use crate::session::{BlockPart, Scroll};
    use crate::snapshot::{Palette, Point};

    fn own() -> Own {
        let mut terminal = Terminal::new(Size { columns: 20, lines: 4 }, 100);
        for (n, code) in [(0, 0), (1, 2), (2, 0)] {
            terminal.advance(format!("\x1b]133;A\x07$ \x1b]133;B\x07cmd{n}\r\n\x1b]133;C\x07").as_bytes());
            for line in 0..3 {
                terminal.advance(format!("out{n}.{line}\r\n").as_bytes());
            }
            terminal.advance(format!("\x1b]133;D;{code}\x07").as_bytes());
        }
        terminal.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        Own::new(terminal)
    }

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

    #[test]
    fn the_view_lists_its_blocks_with_a_sticky_header() {
        let mut own = own();
        own.scroll(Scroll::Lines(4));
        let view = own.blocks_view();
        assert_eq!(rows(&own), ["out1.0", "out1.1", "out1.2", "$ cmd2"]);
        let sticky = view.sticky.expect("cmd1's prompt is above the view");
        assert_eq!(sticky.heading, "$ cmd1");
        assert_eq!(sticky.block.exit_code, Some(2));
        assert_eq!(view.blocks.len(), 2);
        assert_eq!(view.blocks[1].prompt_line, Some(3));
        assert_eq!(
            own.current_block(),
            Some(sticky.block.id),
            "the block at the top when scrolled back"
        );
        assert_eq!(
            own.block_text(sticky.block.id, BlockPart::Output).as_deref(),
            Some("out1.0\nout1.1\nout1.2")
        );
        assert_eq!(
            own.block_text(sticky.block.id, BlockPart::Command).as_deref(),
            Some("cmd1")
        );
    }

    #[test]
    fn collapsing_hides_the_output_from_the_view_and_scrolling() {
        let mut own = own();
        let blocks = own.block_list();
        let newest = own.current_block().expect("cmd2 is the newest command");
        assert_eq!(newest, blocks[2].id);
        assert_eq!(own.toggle_fold(blocks[3].id), None, "a prompt has nothing to collapse");
        assert_eq!(own.toggle_fold(blocks[2].id), Some(true));
        assert_eq!(rows(&own), ["out1.1", "out1.2", "$ cmd2", "$"]);
        assert_eq!(own.toggle_fold(blocks[1].id), Some(true));
        assert_eq!(rows(&own), ["out0.2", "$ cmd1", "$ cmd2", "$"]);
        own.scroll(Scroll::Lines(1));
        assert_eq!(rows(&own), ["out0.1", "out0.2", "$ cmd1", "$ cmd2"]);
        own.scroll(Scroll::Lines(-1));
        assert_eq!(rows(&own), ["out0.2", "$ cmd1", "$ cmd2", "$"]);
        assert_eq!(own.terminal.grid().display_offset, 0, "back at the live screen");
        let snapshot = own.snapshot(&Palette::default());
        assert_eq!(snapshot.cursor.point.line, 3);
        let view = own.blocks_view();
        assert!(view.blocks.iter().any(|block| block.folded && block.output_lines == 3));
        assert_eq!(own.toggle_fold(blocks[1].id), Some(false));
        assert_eq!(rows(&own), ["out1.1", "out1.2", "$ cmd2", "$"]);
    }

    #[test]
    fn selecting_a_block_selects_its_lines_and_a_click_moves_the_prompt_cursor() {
        let mut own = own();
        let id = own.block_list()[0].id;
        assert!(own.select_block(id));
        let text = own.selected_text().unwrap_or_default();
        assert_eq!(text.trim_end(), "$ cmd0\nout0.0\nout0.1\nout0.2");
        assert_eq!(own.current_block(), Some(id));
        own.scroll(Scroll::Bottom);
        own.selection = None;
        own.terminal.advance(b"ls -l");
        assert_eq!(own.prompt_steps(Point { line: 3, column: 3 }), Some(-4));
        assert_eq!(own.prompt_steps(Point { line: 2, column: 3 }), None);
    }

    #[test]
    fn a_bookmark_goes_on_the_cursors_line_and_is_found_again() {
        let mut own = own();
        assert!(own.toggle_bookmark_here());
        assert_eq!(own.blocks_view().bookmarks, [3]);
        own.scroll(Scroll::Top);
        assert!(own.jump_to_bookmark(1));
        assert_eq!(own.terminal.grid().display_offset, 0);
    }
}
