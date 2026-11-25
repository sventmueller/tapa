//! Mac keys as the terminal's keys: the virtual key codes of keys that type nothing, the keypad, the modifier keys,
//! and when ⌥ acts as Meta.
//!
//! ⌥ types the character the keyboard layout puts on it, as everywhere on a Mac: German layouts need it for `@`, `[`,
//! `|` and `~`, and dead keys such as ⌥E or ⌥U compose. ⌥ is Meta (`ESC` before the key) only where it composes
//! nothing: on keys that type no character, such as ⌥⌫ or ⌥←, and on character keys for which the layout has no ⌥
//! character of its own. The setting "Wahltaste als Meta-Taste" makes ⌥ Meta for every key, as in Terminal.app.

use term_session::NamedKey;
use term_session::input::{ModifierKey, Side};

/// The key with virtual key code `code` if it types no character, such as Return or F5.
pub(crate) fn named(code: u16) -> Option<NamedKey> {
    Some(match code {
        0x24 | 0x4c => NamedKey::Enter,
        0x30 => NamedKey::Tab,
        0x33 => NamedKey::Backspace,
        0x35 => NamedKey::Escape,
        0x75 => NamedKey::Delete,
        // The Help key sits where PC keyboards have Insert.
        0x72 => NamedKey::Insert,
        0x73 => NamedKey::Home,
        0x77 => NamedKey::End,
        0x74 => NamedKey::PageUp,
        0x79 => NamedKey::PageDown,
        0x7b => NamedKey::Left,
        0x7c => NamedKey::Right,
        0x7d => NamedKey::Down,
        0x7e => NamedKey::Up,
        0x6e => NamedKey::Menu,
        0x47 => NamedKey::NumLock,
        code => NamedKey::F(function_number(code)?),
    })
}

fn function_number(code: u16) -> Option<u8> {
    const KEYS: [u16; 20] = [
        0x7a, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6d, 0x67, 0x6f, 0x69, 0x6b, 0x71, 0x6a, 0x40, 0x4f,
        0x50, 0x5a,
    ];
    let index = KEYS.iter().position(|&key| key == code)?;
    u8::try_from(index + 1).ok()
}

/// Whether `code` is a key of the numeric keypad.
pub(crate) fn is_keypad(code: u16) -> bool {
    matches!(
        code,
        0x41 | 0x43 | 0x45 | 0x4b | 0x4c | 0x4e | 0x51 | 0x52..=0x59 | 0x5b | 0x5c
    )
}

/// The modifier key with virtual key code `code`.
pub(crate) fn modifier(code: u16) -> Option<(ModifierKey, Side)> {
    Some(match code {
        0x38 => (ModifierKey::Shift, Side::Left),
        0x3c => (ModifierKey::Shift, Side::Right),
        0x3b => (ModifierKey::Control, Side::Left),
        0x3e => (ModifierKey::Control, Side::Right),
        0x3a => (ModifierKey::Alt, Side::Left),
        0x3d => (ModifierKey::Alt, Side::Right),
        0x37 => (ModifierKey::Command, Side::Left),
        0x36 => (ModifierKey::Command, Side::Right),
        _ => return None,
    })
}

/// Whether ⌥ acts as Meta on a character key that types `composed` with ⌥ and `plain` without it.
pub(crate) fn option_is_meta(composed: &str, plain: &str) -> bool {
    !composed.is_empty() && composed == plain
}

/// The bytes ⌥ (`alt`) or ⌘ with `key` send at a shell prompt: a word or the whole line left or right, and deleting
/// the word or the line before the cursor; readline, zle and fish all read these.
pub(crate) fn text_navigation(key: NamedKey, alt: bool) -> Option<&'static [u8]> {
    match (key, alt) {
        (NamedKey::Left, true) => Some(b"\x1bb"),
        (NamedKey::Right, true) => Some(b"\x1bf"),
        (NamedKey::Backspace, true) => Some(b"\x1b\x7f"),
        (NamedKey::Left, false) => Some(b"\x01"),
        (NamedKey::Right, false) => Some(b"\x05"),
        (NamedKey::Backspace, false) => Some(b"\x15"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_moves_by_words_and_command_by_lines() {
        assert_eq!(text_navigation(NamedKey::Left, true), Some(&b"\x1bb"[..]));
        assert_eq!(text_navigation(NamedKey::Right, false), Some(&b"\x05"[..]));
        assert_eq!(text_navigation(NamedKey::Backspace, false), Some(&b"\x15"[..]));
        assert_eq!(text_navigation(NamedKey::Up, true), None);
    }

    #[test]
    fn keys_that_type_nothing_have_names_and_letters_do_not() {
        assert_eq!(named(0x24), Some(NamedKey::Enter));
        assert_eq!(named(0x4c), Some(NamedKey::Enter));
        assert_eq!(named(0x7a), Some(NamedKey::F(1)));
        assert_eq!(named(0x5a), Some(NamedKey::F(20)));
        assert_eq!(named(0x7e), Some(NamedKey::Up));
        assert_eq!(named(0x00), None, "A");
        assert_eq!(named(0x31), None, "space types a character");
        assert!(is_keypad(0x52) && is_keypad(0x4c) && !is_keypad(0x24));
        assert_eq!(modifier(0x3d), Some((ModifierKey::Alt, Side::Right)));
    }

    #[test]
    fn every_key_of_the_editing_block_and_the_arrows_has_its_name() {
        let keys = [
            (0x30, NamedKey::Tab),
            (0x33, NamedKey::Backspace),
            (0x35, NamedKey::Escape),
            (0x75, NamedKey::Delete),
            (0x72, NamedKey::Insert),
            (0x73, NamedKey::Home),
            (0x77, NamedKey::End),
            (0x74, NamedKey::PageUp),
            (0x79, NamedKey::PageDown),
            (0x7b, NamedKey::Left),
            (0x7c, NamedKey::Right),
            (0x7d, NamedKey::Down),
            (0x6e, NamedKey::Menu),
            (0x47, NamedKey::NumLock),
            (0x60, NamedKey::F(5)),
            (0x6f, NamedKey::F(12)),
        ];
        for (code, key) in keys {
            assert_eq!(named(code), Some(key), "{code:#x}");
        }
    }

    #[test]
    fn both_sides_of_every_modifier_key_are_told_apart() {
        let keys = [
            (0x38, ModifierKey::Shift, Side::Left),
            (0x3c, ModifierKey::Shift, Side::Right),
            (0x3b, ModifierKey::Control, Side::Left),
            (0x3e, ModifierKey::Control, Side::Right),
            (0x3a, ModifierKey::Alt, Side::Left),
            (0x37, ModifierKey::Command, Side::Left),
            (0x36, ModifierKey::Command, Side::Right),
        ];
        for (code, key, side) in keys {
            assert_eq!(modifier(code), Some((key, side)), "{code:#x}");
        }
        assert_eq!(modifier(0x39), None, "Caps Lock");
        assert_eq!(modifier(0x00), None);
    }

    #[test]
    fn option_composes_where_the_layout_has_a_character_and_is_meta_elsewhere() {
        assert!(!option_is_meta("@", "l"), "⌥L on a German layout");
        assert!(!option_is_meta("", "e"), "a dead key composes with the next one");
        assert!(option_is_meta("b", "b"), "a layout without a ⌥ character");
    }
}
