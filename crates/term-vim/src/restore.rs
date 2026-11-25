//! What Vim brings back and `modalkit` does not: the last selection for `gv`, and for `U` the line the latest changes
//! went to, as it was before them.

use modalkit::editing::cursor::{Cursor, CursorGroup, CursorState};

use crate::{Effect, Mode, Replacement, Vim};

impl Vim {
    /// Keeps the selection while Visual mode is on, for `gv`.
    pub(crate) fn remember_selection(&mut self) {
        let mode = self.mode();
        if matches!(mode, Mode::Visual | Mode::VisualLine | Mode::VisualBlock) {
            let state = self.buffer.get_leader_state(self.group).clone();
            self.last_selection = Some((state, mode));
        }
    }

    /// Whether `key` after the keys pending makes `gv`, which `modalkit` leaves unmapped.
    pub(crate) fn is_reselect(&self, key: &str) -> bool {
        key == "v"
            && self.last_selection.is_some()
            && self
                .pending
                .split_last()
                .is_some_and(|(g, count)| g == "g" && count.iter().all(|k| k.chars().all(|c| c.is_ascii_digit())))
    }

    /// `gv`: the last selection again, in the mode it had.
    pub(crate) fn reselect(&mut self, effects: &mut Vec<Effect>) {
        let Some((state, mode)) = self.last_selection.clone() else {
            return;
        };
        let enter = match mode {
            Mode::VisualLine => "V",
            Mode::VisualBlock => "<C-V>",
            _ => "v",
        };
        for key in crate::parse(&format!("<Esc>{enter}")) {
            self.feed(key, effects);
        }
        let state = match state {
            CursorState::Selection(cursor, anchor, shape) => {
                CursorState::Selection(self.clamped(&cursor), self.clamped(&anchor), shape)
            }
            CursorState::Location(cursor) => CursorState::Location(self.clamped(&cursor)),
        };
        self.buffer.set_group(self.group, CursorGroup::new(state, Vec::new()));
    }

    /// Takes `text` as the text, the first `gone` lines of the old one no longer in it, keeping the mode and moving
    /// the cursor and the selection with the lines they were on, as for a terminal whose history drops its oldest.
    pub fn replace_lines(&mut self, text: &str, gone: usize) {
        let up = |cursor: &Cursor| Cursor::new(cursor.y.saturating_sub(gone), cursor.x);
        let state = match self.buffer.get_leader_state(self.group).clone() {
            CursorState::Selection(cursor, anchor, shape) => CursorState::Selection(up(&cursor), up(&anchor), shape),
            CursorState::Location(cursor) => CursorState::Location(up(&cursor)),
        };
        let (copy, added_break) = crate::shadow(text);
        self.added_break = added_break;
        self.buffer.set_text(copy);
        let state = match state {
            CursorState::Selection(cursor, anchor, shape) => {
                CursorState::Selection(self.clamped(&cursor), self.clamped(&anchor), shape)
            }
            CursorState::Location(cursor) => CursorState::Location(self.clamped(&cursor)),
        };
        self.buffer.set_group(self.group, CursorGroup::new(state, Vec::new()));
        self.last_selection = None;
        self.line_undo = None;
    }

    /// `cursor` within the text as it is now.
    fn clamped(&self, cursor: &Cursor) -> Cursor {
        let line = cursor.y.min(self.buffer.get_lines().saturating_sub(1));
        Cursor::new(line, cursor.x.min(self.buffer.get_columns(line)))
    }

    /// Takes note of `change`, made to `before`, for `U`: changes within one line keep its text from before the first
    /// of them; any other change forgets it.
    pub(crate) fn note_line_change(&mut self, before: &ropey::Rope, change: &Replacement) {
        let line = change.start.0;
        if change.end.0 != line || change.text.contains('\n') {
            self.line_undo = None;
            return;
        }
        if self.line_undo.as_ref().is_some_and(|(noted, _)| *noted == line) {
            return;
        }
        self.line_undo = Some((line, line_text(before, line)));
    }

    /// `U`: the line of the latest changes as it was before them, itself a change that a second `U` takes back;
    /// whether the text changed.
    pub(crate) fn undo_line(&mut self) -> bool {
        let Some((line, original)) = self.line_undo.clone() else {
            return false;
        };
        let copy = self.buffer.get().as_rope().clone();
        if line >= self.buffer.get_lines() {
            self.line_undo = None;
            return false;
        }
        let current = line_text(&copy, line);
        if current == original {
            return false;
        }
        let start = copy.line_to_char(line);
        let mut changed = copy;
        changed.remove(start..start + current.chars().count());
        changed.insert(start, &original);
        let cursor = self.cursor();
        self.buffer.set_text(changed.to_string());
        let last = original.chars().count().saturating_sub(1);
        self.set_cursor(cursor.0, cursor.1.min(last));
        self.line_undo = Some((line, current));
        true
    }
}

/// Line `line` of `rope`, without its break.
fn line_text(rope: &ropey::Rope, line: usize) -> String {
    let mut text = rope.get_line(line).map(|l| l.to_string()).unwrap_or_default();
    if text.ends_with('\n') {
        text.pop();
    }
    text
}
