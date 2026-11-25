//! The keyboard: `keyDown:` turned into the terminal's keys, and text from input methods and dead keys through
//! `NSTextInputClient`.
//!
//! Keys that type nothing, the keypad, ⌃ and ⌥ as Meta go straight to the program. Keys that type text go through
//! `interpretKeyEvents:`, so input methods can compose; a key no input method answers is sent as it is. ⌘ belongs to
//! the menus: a ⌘ key reaches the view only when no menu item took it, and then only programs that asked for ⌘ via the
//! kitty keyboard protocol get it.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, Message, sel};
use objc2_app_kit::{NSCursor, NSEvent, NSEventModifierFlags};
use objc2_foundation::{NSArray, NSAttributedString, NSRange, NSRect, NSString, NSUInteger};
use term_session::input::ModifierKey;
use term_session::{Key, KeyAction, KeyCode, Modifiers, NamedKey, Scroll};

use super::view::TerminalView;
use crate::keys;

/// AppKit's `NSNotFound`, which ranges use to say "none".
pub(crate) const NOT_FOUND: NSUInteger = NSUInteger::MAX >> 1;

#[derive(Default)]
pub(crate) struct KeyState {
    /// Text an input method is composing.
    pub(crate) marked: RefCell<String>,
    /// The part of `marked` the input method selected, as location and length.
    selection: Cell<(usize, usize)>,
    /// The key being interpreted while `interpretKeyEvents:` runs.
    event: RefCell<Option<Retained<NSEvent>>>,
    /// An input method or the text system answered the key being interpreted.
    answered: Cell<bool>,
    /// The interval from the first key since the last snapshot of the screen until the frame that draws the next one.
    pub(crate) typed: RefCell<Option<term_base::signpost::Interval>>,
    /// That interval once the snapshot is taken, until its frame is drawn.
    pub(crate) typed_shown: RefCell<Option<term_base::signpost::Interval>>,
}

fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        command: flags.contains(NSEventModifierFlags::Command),
    }
}

fn string(value: Option<Retained<NSString>>) -> String {
    value.map(|s| s.to_string()).unwrap_or_default()
}

fn action_of(event: &NSEvent) -> KeyAction {
    if event.isARepeat() {
        KeyAction::Repeat
    } else {
        KeyAction::Press
    }
}

/// The terminal's key for `event`, with ⌥ as Meta where the setting or `keys::option_is_meta` says so; `None` for a
/// key without any character or name.
fn key_of(event: &NSEvent, action: KeyAction) -> Option<Key> {
    let flags = event.modifierFlags();
    let mut modifiers = modifiers(flags);
    let code = event.keyCode();
    let keypad = keys::is_keypad(code);
    if let Some(named) = keys::named(code) {
        return Some(Key {
            code: KeyCode::Named(named),
            modifiers,
            text: None,
            keypad,
            action,
        });
    }
    let base = string(event.charactersByApplyingModifiers(NSEventModifierFlags::empty()))
        .chars()
        .next()
        .or_else(|| string(event.charactersIgnoringModifiers()).chars().next())?;
    let plain = string(event.charactersByApplyingModifiers(flags & NSEventModifierFlags::Shift));
    let typed = string(event.characters());
    let text = if modifiers.control || modifiers.command {
        plain
    } else if modifiers.alt && !super::option_as_meta() && !keys::option_is_meta(&typed, &plain) {
        modifiers.alt = false;
        typed
    } else if modifiers.alt {
        plain
    } else {
        typed
    };
    Some(Key {
        code: KeyCode::Char(base.to_lowercase().next().unwrap_or(base)),
        modifiers,
        text: (!text.is_empty() && !text.chars().any(char::is_control)).then_some(text),
        keypad,
        action,
    })
}

/// The chord of a key press as the settings file writes it, for matching it against the key bindings.
pub fn chord_of(event: &NSEvent) -> Option<term_config::Chord> {
    let flags = event.modifierFlags();
    let name = match keys::named(event.keyCode()) {
        Some(named) => match named {
            NamedKey::Enter => "enter".to_string(),
            NamedKey::Tab => "tab".into(),
            NamedKey::Backspace => "backspace".into(),
            NamedKey::Escape => "escape".into(),
            NamedKey::Up => "up".into(),
            NamedKey::Down => "down".into(),
            NamedKey::Left => "left".into(),
            NamedKey::Right => "right".into(),
            NamedKey::Home => "home".into(),
            NamedKey::End => "end".into(),
            NamedKey::PageUp => "pageup".into(),
            NamedKey::PageDown => "pagedown".into(),
            NamedKey::Delete => "delete".into(),
            NamedKey::F(n) if n <= 12 => format!("f{n}"),
            _ => return None,
        },
        None => {
            let plain = string(event.charactersByApplyingModifiers(NSEventModifierFlags::empty()));
            let key = plain.chars().next()?;
            if key == ' ' {
                "space".into()
            } else {
                key.to_lowercase().collect()
            }
        }
    };
    Some(term_config::Chord {
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        shift: flags.contains(NSEventModifierFlags::Shift),
        cmd: flags.contains(NSEventModifierFlags::Command),
        key: name,
    })
}

/// The key a text-system command stands for, when one arrives outside a key press.
fn key_for_command(selector: Sel) -> Option<Key> {
    let named = [
        (sel!(insertNewline:), NamedKey::Enter),
        (sel!(insertTab:), NamedKey::Tab),
        (sel!(deleteBackward:), NamedKey::Backspace),
        (sel!(deleteForward:), NamedKey::Delete),
        (sel!(cancelOperation:), NamedKey::Escape),
        (sel!(moveUp:), NamedKey::Up),
        (sel!(moveDown:), NamedKey::Down),
        (sel!(moveLeft:), NamedKey::Left),
        (sel!(moveRight:), NamedKey::Right),
    ];
    named
        .into_iter()
        .find(|(command, _)| *command == selector)
        .map(|(_, key)| Key::named(key))
}

/// The text of an `NSString` or `NSAttributedString` an input method hands over.
fn text_of(object: &AnyObject) -> String {
    if let Some(string) = object.downcast_ref::<NSString>() {
        return string.to_string();
    }
    object
        .downcast_ref::<NSAttributedString>()
        .map(|attributed| attributed.string().to_string())
        .unwrap_or_default()
}

impl TerminalView {
    fn send(&self, key: &Key) -> bool {
        self.with_session(|session| session.key(key)).unwrap_or(false)
    }

    /// Handles a key press; `false` leaves it to AppKit, which beeps.
    pub(super) fn key_down(&self, event: &NSEvent) -> bool {
        if self.ivars().session.borrow().is_none() {
            return true;
        }
        NSCursor::setHiddenUntilMouseMoves(true);
        if self.copy_mode_key(event) {
            return true;
        }
        if let Some(renderer) = &self.ivars().renderer {
            renderer.key_pressed();
        }
        let mut typed = self.ivars().keys.typed.borrow_mut();
        if typed.is_none() {
            *typed = term_base::signpost::begin(term_base::signpost::name!("term-key"));
        }
        drop(typed);
        self.wake_cursor();
        let action = action_of(event);
        if !self.has_marked()
            && action != KeyAction::Release
            && let Some(bytes) = self.text_navigation(event)
        {
            self.with_session(|session| {
                session.scroll(Scroll::Bottom);
                session.write(bytes.to_vec());
            });
            return true;
        }
        if !self.has_marked() {
            if event.modifierFlags().contains(NSEventModifierFlags::Command) {
                return key_of(event, action).is_some_and(|key| self.send(&key));
            }
            if let Some(key) = key_of(event, action).filter(|key| {
                matches!(key.code, KeyCode::Named(_)) || key.keypad || key.modifiers.control || key.modifiers.alt
            }) {
                self.send(&key);
                return true;
            }
        }
        self.interpret(event, action);
        true
    }

    /// What ⌥ and ⌘ with the arrows and ⌫ send at a shell prompt, as in text fields on a Mac (F214); none in a
    /// full-screen program, which reads those keys itself.
    fn text_navigation(&self, event: &NSEvent) -> Option<&'static [u8]> {
        let alternate = self.with_session(|session| session.input_modes().alternate_screen)?;
        if alternate {
            return None;
        }
        let flags = event.modifierFlags();
        let held = |flag| flags.contains(flag);
        let alt = held(NSEventModifierFlags::Option);
        let command = held(NSEventModifierFlags::Command);
        if held(NSEventModifierFlags::Control) || held(NSEventModifierFlags::Shift) || alt == command {
            return None;
        }
        keys::text_navigation(keys::named(event.keyCode())?, alt)
    }

    fn interpret(&self, event: &NSEvent, action: KeyAction) {
        let keys = &self.ivars().keys;
        let composing = self.has_marked();
        *keys.event.borrow_mut() = Some(event.retain());
        keys.answered.set(false);
        self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        keys.event.borrow_mut().take();
        if !keys.answered.get()
            && !composing
            && !self.has_marked()
            && let Some(key) = key_of(event, action)
        {
            self.send(&key);
        }
    }

    pub(super) fn key_up(&self, event: &NSEvent) {
        if !self.has_marked()
            && !self.in_copy_mode()
            && let Some(key) = key_of(event, KeyAction::Release)
        {
            self.send(&key);
        }
    }

    /// Modifier keys alone, which only programs using the kitty protocol's "all keys" hear of.
    pub(super) fn flags_changed(&self, event: &NSEvent) {
        self.update_pointer(event.modifierFlags(), None);
        let Some((modifier, side)) = keys::modifier(event.keyCode()) else {
            return;
        };
        let flags = event.modifierFlags();
        let held = flags.contains(match modifier {
            ModifierKey::Shift => NSEventModifierFlags::Shift,
            ModifierKey::Control => NSEventModifierFlags::Control,
            ModifierKey::Alt => NSEventModifierFlags::Option,
            ModifierKey::Command => NSEventModifierFlags::Command,
        });
        self.send(&Key {
            code: KeyCode::Named(NamedKey::Modifier(modifier, side)),
            modifiers: modifiers(flags),
            text: None,
            keypad: false,
            action: if held { KeyAction::Press } else { KeyAction::Release },
        });
    }

    /// Text an input method or a plain key typed.
    pub(super) fn insert(&self, string: &AnyObject) {
        let text = text_of(string);
        let keys = &self.ivars().keys;
        let composed = self.has_marked();
        keys.answered.set(true);
        self.clear_marked();
        if text.is_empty() {
            return;
        }
        // A key that typed its own character goes as that key, so programs using the kitty protocol learn which.
        let event = keys.event.borrow().clone();
        let typed = event
            .filter(|_| !composed)
            .and_then(|event| key_of(&event, action_of(&event)))
            .filter(|key| key.text.as_deref() == Some(text.as_str()));
        if let Some(key) = typed {
            self.send(&key);
            return;
        }
        let scrolled = self
            .ivars()
            .screen
            .borrow()
            .as_ref()
            .is_some_and(|s| s.display_offset > 0);
        self.with_session(|session| {
            if scrolled {
                session.scroll(Scroll::Bottom);
            }
            session.write(text.into_bytes());
        });
    }

    /// A command the text system made of a key, such as `insertNewline:` for Return.
    pub(super) fn command(&self, selector: Sel) {
        let keys = &self.ivars().keys;
        keys.answered.set(true);
        let event = keys.event.borrow().clone();
        let key = match event {
            Some(event) => key_of(&event, action_of(&event)),
            None => key_for_command(selector),
        };
        if let Some(key) = key {
            self.send(&key);
        }
    }

    pub(super) fn set_marked(&self, string: &AnyObject, selected: NSRange) {
        let keys = &self.ivars().keys;
        keys.answered.set(true);
        *keys.marked.borrow_mut() = text_of(string);
        keys.selection.set((selected.location, selected.length));
        self.invalidate_cursor();
    }

    pub(super) fn unmark(&self) {
        self.clear_marked();
    }

    /// Ends a composition the view no longer shows, as when it loses the focus.
    pub(super) fn discard_marked(&self) {
        if self.has_marked() {
            if let Some(context) = self.inputContext() {
                context.discardMarkedText();
            }
            self.clear_marked();
        }
    }

    fn clear_marked(&self) {
        let had = !std::mem::take(&mut *self.ivars().keys.marked.borrow_mut()).is_empty();
        if had {
            self.invalidate_cursor();
        }
    }

    pub(super) fn has_marked(&self) -> bool {
        !self.ivars().keys.marked.borrow().is_empty()
    }

    pub(super) fn marked_selection(&self) -> NSRange {
        if !self.has_marked() {
            return NSRange::new(NOT_FOUND, 0);
        }
        let (location, length) = self.ivars().keys.selection.get();
        NSRange::new(location, length)
    }

    pub(super) fn marked_range_now(&self) -> NSRange {
        let length = self.ivars().keys.marked.borrow().encode_utf16().count();
        if length == 0 {
            NSRange::new(NOT_FOUND, 0)
        } else {
            NSRange::new(0, length)
        }
    }

    /// The cursor's cell on the screen, where input methods put their candidate window.
    pub(super) fn cursor_on_screen(&self) -> NSRect {
        let point = self
            .ivars()
            .screen
            .borrow()
            .as_ref()
            .map(|s| s.cursor.point)
            .unwrap_or_default();
        let in_view = self.metrics().cell_rect(point, 1);
        let in_window = self.convertRect_toView(in_view, None);
        self.window()
            .map_or(in_window, |window| window.convertRectToScreen(in_window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_commands_outside_a_key_press_still_reach_the_program() {
        assert_eq!(key_for_command(sel!(insertNewline:)), Some(Key::named(NamedKey::Enter)));
        assert_eq!(
            key_for_command(sel!(cancelOperation:)),
            Some(Key::named(NamedKey::Escape))
        );
        assert_eq!(key_for_command(sel!(noop:)), None);
        assert_eq!(NOT_FOUND, isize::MAX.unsigned_abs());
    }
}
