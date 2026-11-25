//! The self-test's hooks of `mouse.rs`.

use super::*;

impl TerminalView {
    /// Whether the pointing hand shows, for the self-test.
    pub(crate) fn pointing_hand(&self) -> bool {
        self.ivars().mouse.hand.get()
    }
}

impl TerminalView {
    /// Selects what a double click at screen cell `point` selects and returns its text, for the self-test.
    pub fn double_click_at(&self, point: Point) -> Option<String> {
        self.with_session(term_session::Session::clear_selection);
        if !self.select_smart(point) {
            self.with_session(|session| session.select(SelectionKind::Word, point, CellSide::Left));
        }
        self.refresh(true);
        self.selected_text()
    }

    /// Selects the rectangle between screen cells `from` and `to` as ⌥-drag does and returns its text.
    pub fn block_select(&self, from: Point, to: Point) -> Option<String> {
        self.with_session(|session| {
            session.select(SelectionKind::Block, from, CellSide::Left);
            session.extend_selection(to, CellSide::Right);
        });
        self.refresh(true);
        self.selected_text()
    }
}
