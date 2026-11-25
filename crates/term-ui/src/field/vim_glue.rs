//! The field's text view with Vim keys (F95): turns key presses into `VimKey`s, hands them to the field's Vim state
//! and shows what it answers, its mode in the grey hint.

use objc2::DefinedClass;
use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use objc2_foundation::NSRange;
use term_session::NamedKey;

use super::FieldText;
use super::vim::{Answer, VimKey};
use crate::TerminalView;

/// The Vim key of a key press; none for one with ⌘, which belongs to the menus, or one Vim has no use for.
fn key_of(event: &NSEvent) -> Option<VimKey> {
    let flags = event.modifierFlags();
    if flags.contains(NSEventModifierFlags::Command) {
        return None;
    }
    if let Some(named) = crate::keys::named(event.keyCode()) {
        return Some(match named {
            NamedKey::Escape => VimKey::Escape,
            NamedKey::Enter => VimKey::Enter,
            NamedKey::Backspace => VimKey::Backspace,
            NamedKey::Up => VimKey::Up,
            NamedKey::Down => VimKey::Down,
            NamedKey::Left => VimKey::Left,
            NamedKey::Right => VimKey::Right,
            NamedKey::Home => VimKey::Home,
            NamedKey::End => VimKey::End,
            _ => return None,
        });
    }
    if flags.contains(NSEventModifierFlags::Control) {
        let plain = event.charactersIgnoringModifiers()?.to_string();
        return plain.chars().next().map(|c| VimKey::Ctrl(c.to_ascii_lowercase()));
    }
    event.characters()?.to_string().chars().next().map(VimKey::Char)
}

/// The UTF-16 offset of the character `chars` characters into `text`, as the text view counts.
fn utf16_of(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

/// The characters before the UTF-16 offset `utf16` in `text`.
fn chars_of(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    text.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= utf16
        })
        .count()
}

impl FieldText {
    /// Handles `event` with Vim's keys while the setting asks for them; `None` for a key the field handles as
    /// without Vim, as most are in Insert mode.
    pub(super) fn vim_key(&self, view: &TerminalView, event: &NSEvent) -> Option<bool> {
        if !crate::host().config().input.vim {
            return None;
        }
        let key = key_of(event)?;
        let normal = view.ivars().field_state.borrow().vim.mode() != term_vim::Mode::Insert;
        if normal && key == VimKey::Ctrl('c') {
            view.ivars().field_state.borrow_mut().vim.reset();
            self.set_text("");
            return Some(true);
        }
        self.vim_feed(view, key)
    }

    /// Feeds `key` to the field's Vim state and carries out the answer.
    pub(super) fn vim_feed(&self, view: &TerminalView, key: VimKey) -> Option<bool> {
        let text = self.text();
        let caret = chars_of(&text, self.caret());
        let answer = view.ivars().field_state.borrow_mut().vim.key(key, &text, caret);
        match answer {
            Answer::Pass => return None,
            Answer::Submit => view.field_submit(),
            Answer::History(older) => {
                view.field_history(older);
                let text = self.text();
                let length = text.chars().count();
                let mut state = view.ivars().field_state.borrow_mut();
                state.vim.reset();
                let answer = state.vim.key(VimKey::Escape, &text, length);
                drop(state);
                self.vim_show(view, answer);
            }
            answer @ Answer::Show { .. } => self.vim_show(view, answer),
        }
        Some(true)
    }

    fn vim_show(&self, view: &TerminalView, answer: Answer) {
        let Answer::Show {
            text,
            caret,
            selected,
            copy,
        } = answer
        else {
            return;
        };
        if text != self.text() {
            self.set_text(&text);
        }
        let start = utf16_of(&text, caret);
        let end = utf16_of(&text, caret + selected);
        self.setSelectedRange(NSRange::new(start, end - start));
        if let Some(copy) = copy {
            crate::clip::store(&copy);
        }
        let status = view.ivars().field_state.borrow().vim.status();
        if let Some(status) = status
            && let Ok(field) = view.ivars().field.try_borrow()
            && let Some(field) = field.as_ref()
        {
            field.set_hint(Some(&status));
        }
    }
}

impl TerminalView {
    /// Feeds `keys`, written as Vim writes them, to the field as if the setting asked for Vim keys, typing what Insert
    /// mode passes on; says the field's text, selection and mode.
    #[cfg(feature = "selftest")]
    pub fn field_vim_keys(&self, keys: &str) -> String {
        let Some(text_view) = self.ivars().field.borrow().as_ref().map(|field| field.text.clone()) else {
            return "no field".into();
        };
        for key in super::vim::parse(keys) {
            if text_view.vim_feed(self, key).is_none()
                && let VimKey::Char(c) = key
            {
                let text = text_view.text();
                let at = chars_of(&text, text_view.caret());
                let mut chars: Vec<char> = text.chars().collect();
                chars.insert(at.min(chars.len()), c);
                let typed: String = chars.into_iter().collect();
                text_view.set_text(&typed);
                text_view.setSelectedRange(NSRange::new(utf16_of(&typed, at + 1), 0));
            }
        }
        let range = text_view.selectedRange();
        format!(
            "text {:?} selection {}+{} mode {:?}",
            text_view.text(),
            range.location,
            range.length,
            self.ivars().field_state.borrow().vim.mode()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_turn_between_characters_and_utf16() {
        let text = "a😀b";
        assert_eq!(utf16_of(text, 2), 3);
        assert_eq!(chars_of(text, 3), 2);
        assert_eq!(chars_of(text, 0), 0);
        assert_eq!(chars_of(text, 99), 3);
    }
}
