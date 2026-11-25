//! The suggestion without the input field (E7, F94): what the history or the spec suggests for the line typed at the
//! prompt shows faint from the cursor on, as Warp draws it; →, ⌃F, ⌃E and End take it as typed keys, Tab where it is
//! the one line meant, ⌃→ its next word. Tab otherwise or ⌃Space opens the completion menu above the word. A shell
//! that draws suggestions of its own, such as fish or zsh with zsh-autosuggestions, keeps those, and Tab with them.

use objc2::DefinedClass;
use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use term_session::{Point, Scroll};

use super::model::Phase;
use crate::TerminalView;

/// How long a spec the suggestion needs has to be parsed before the suggestion is looked for again.
const SPEC_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// The suggestion drawn from the cursor on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ghost {
    /// Where the cursor was in the view when the suggestion was made; it shows only while the cursor is there.
    pub(crate) point: Point,
    /// What shows, cut to the room after the cursor.
    pub(crate) shown: String,
    /// The whole rest of the suggested line, which taking it types.
    pub(crate) rest: String,
    /// The line typed when the suggestion was made; it is taken only while the line is still that.
    typed: String,
    /// It is the one line meant, from the history or the only candidate, so Tab takes it.
    sure: bool,
}

/// The rest's next word with the spaces before it, as ⌃→ takes it.
fn next_word(rest: &str) -> &str {
    let spaces = rest.len() - rest.trim_start().len();
    let word = rest[spaces..].find(char::is_whitespace).unwrap_or(rest.len() - spaces);
    &rest[..spaces + word]
}

impl TerminalView {
    /// Looks for the suggestion anew after the screen, the shell or the field changed, and draws it; an open menu
    /// follows the line instead, or closes once the prompt is gone.
    pub(crate) fn raw_suggest(&self) {
        if self.raw_menu_is_open() {
            match self.raw_line() {
                Some(text) => self.menu_follow(&text),
                None => self.menu_close_quietly(),
            }
        }
        let ghost = self.raw_ghost();
        if *self.ivars().ghost.borrow() == ghost {
            return;
        }
        *self.ivars().ghost.borrow_mut() = ghost;
        self.repaint();
    }

    /// Whether the terminal suggests and completes at the shell's prompt now: the shell waits for input, without the
    /// field, and draws no suggestions of its own.
    fn raw_mode_suggests(&self) -> bool {
        crate::host().config().input.suggestions
            && !self.ivars().shell.borrow().own_suggestions
            && !self.field_view().is_some_and(|field| field.is_visible())
            && !self.in_copy_mode()
            && !self.has_marked()
            && self
                .ivars()
                .field_state
                .try_borrow()
                .is_ok_and(|state| state.phase == Phase::Input)
    }

    /// The line typed at the prompt while the terminal suggests there.
    fn raw_line(&self) -> Option<String> {
        if !self.raw_mode_suggests() {
            return None;
        }
        let (typed, _) = self.with_session(|session| session.typed_line()).flatten()?;
        (!typed.text.contains('\n')).then_some(typed.text)
    }

    fn raw_ghost(&self) -> Option<Ghost> {
        if !self.raw_mode_suggests() || self.raw_menu_is_open() {
            return None;
        }
        let (typed, point) = self.with_session(|session| session.typed_line()).flatten()?;
        let text = typed.text;
        if !super::suggest::typed_enough(&text) || text.contains('\n') {
            return None;
        }
        let room = if typed.open {
            typed.room
        } else {
            typed.room.saturating_sub(1)
        };
        if room == 0 {
            return None;
        }
        let (line, missing_spec) = self.instant_line(&text);
        let Some((line, sure)) = line else {
            if let Some(missing) = missing_spec {
                term_suggest::warm_spec(&missing);
                let id = self.id();
                crate::kit::main_after(SPEC_WAIT, move || {
                    if let Some(view) = crate::find(id) {
                        view.raw_suggest();
                    }
                });
            }
            return None;
        };
        let rest = line.strip_prefix(text.as_str())?.lines().next()?.to_string();
        if rest.is_empty() {
            return None;
        }
        let shown = rest.chars().take(room).collect();
        Some(Ghost {
            point,
            shown,
            rest,
            typed: text,
            sure,
        })
    }

    /// A key at the shell's prompt: the open menu's keys first, then →, ⌃F, ⌃E or End take the suggestion that shows,
    /// Tab where it is the one line meant, and ⌃→ its next word; Tab otherwise or ⌃Space completes the word. Whether
    /// the key did.
    /// Where the terminal finds nothing, Tab goes to the shell's own completion.
    pub(crate) fn raw_key(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        if flags.intersects(NSEventModifierFlags::Option | NSEventModifierFlags::Command) {
            return false;
        }
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let control = flags.contains(NSEventModifierFlags::Control);
        if self.raw_menu_is_open() {
            return !control && self.menu_key(event.keyCode(), shift, true).unwrap_or(false);
        }
        if self.raw_accept(event) {
            return true;
        }
        let completes = matches!(
            (event.keyCode(), control, shift),
            (48, false, false) | (49, true, false)
        );
        let completed =
            completes && self.raw_line().is_some_and(|text| !text.trim().is_empty()) && self.complete_word(true);
        if completed {
            self.raw_suggest();
        }
        completed
    }

    fn raw_accept(&self, event: &NSEvent) -> bool {
        let Some(ghost) = self.ivars().ghost.borrow().clone() else {
            return false;
        };
        let shows = self
            .with_session(|session| session.typed_line())
            .flatten()
            .is_some_and(|(typed, point)| typed.text == ghost.typed && point == ghost.point);
        let flags = event.modifierFlags();
        if !shows
            || flags
                .intersects(NSEventModifierFlags::Shift | NSEventModifierFlags::Option | NSEventModifierFlags::Command)
        {
            return false;
        }
        let control = flags.contains(NSEventModifierFlags::Control);
        let key = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let typed = match (event.keyCode(), key.as_str(), control) {
            (124, _, true) => next_word(&ghost.rest).to_string(),
            (48, _, false) if ghost.sure => ghost.rest,
            (124 | 119, _, false) | (_, "f" | "e", true) => ghost.rest,
            _ => return false,
        };
        *self.ivars().ghost.borrow_mut() = None;
        self.repaint();
        self.with_session(|session| {
            session.scroll(Scroll::Bottom);
            session.write(typed.into_bytes());
        });
        true
    }

    /// The suggestion that shows without the field and what it was made from, for the self-test.
    #[cfg(feature = "selftest")]
    pub fn raw_suggestion(&self) -> String {
        self.raw_suggest();
        let typed = self.with_session(|session| session.typed_line()).flatten();
        let shown = self.field_shown();
        let phase = self.ivars().field_state.borrow().phase;
        format!(
            "{:?} (phase {phase:?}, own {}, field {shown}, typed {:?})",
            self.ivars().ghost.borrow().as_ref().map(|ghost| ghost.shown.clone()),
            self.ivars().shell.borrow().own_suggestions,
            typed.map(|(line, point)| (line.text, line.room, line.open, point.line, point.column)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_word_keeps_the_spaces_before_it() {
        assert_eq!(next_word("ommit -m x"), "ommit");
        assert_eq!(next_word(" -m x"), " -m");
        assert_eq!(next_word("x"), "x");
        assert_eq!(next_word(""), "");
    }
}
