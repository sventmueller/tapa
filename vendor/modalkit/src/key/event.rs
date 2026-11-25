//! # Key events
//!
//! Dango: the subset of `crossterm::event` that [TerminalKey](super::TerminalKey) is built on, so that the crate
//! does not need crossterm, whose `event` module only comes with its terminal backend; the `crossterm` feature adds
//! conversions from crossterm's types.
use bitflags::bitflags;

bitflags! {
    /// Modifier keys held down with a key.
    #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
    pub struct KeyModifiers: u8 {
        /// Shift.
        const SHIFT = 0b0000_0001;
        /// Control.
        const CONTROL = 0b0000_0010;
        /// Alt, or Option.
        const ALT = 0b0000_0100;
        /// Super, or Command.
        const SUPER = 0b0000_1000;
        /// Hyper.
        const HYPER = 0b0001_0000;
        /// Meta.
        const META = 0b0010_0000;
        /// No modifier.
        const NONE = 0b0000_0000;
    }
}

/// A media key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd)]
pub enum MediaKeyCode {
    /// Play/Pause.
    PlayPause,
    /// Play.
    Play,
    /// Pause.
    Pause,
    /// Reverse.
    Reverse,
    /// Stop.
    Stop,
    /// Fast-forward.
    FastForward,
    /// Rewind.
    Rewind,
    /// Next track.
    TrackNext,
    /// Previous track.
    TrackPrevious,
    /// Record.
    Record,
    /// Lower volume.
    LowerVolume,
    /// Raise volume.
    RaiseVolume,
    /// Mute volume.
    MuteVolume,
}

/// A modifier key pressed on its own.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd)]
pub enum ModifierKeyCode {
    /// Left Shift.
    LeftShift,
    /// Left Control.
    LeftControl,
    /// Left Alt.
    LeftAlt,
    /// Left Super.
    LeftSuper,
    /// Left Hyper.
    LeftHyper,
    /// Left Meta.
    LeftMeta,
    /// Right Shift.
    RightShift,
    /// Right Control.
    RightControl,
    /// Right Alt.
    RightAlt,
    /// Right Super.
    RightSuper,
    /// Right Hyper.
    RightHyper,
    /// Right Meta.
    RightMeta,
    /// ISO Level 3 Shift.
    IsoLevel3Shift,
    /// ISO Level 5 Shift.
    IsoLevel5Shift,
}

/// A key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd)]
pub enum KeyCode {
    /// Backspace.
    Backspace,
    /// Enter.
    Enter,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Tab.
    Tab,
    /// Shift-Tab.
    BackTab,
    /// Delete.
    Delete,
    /// Insert.
    Insert,
    /// A function key, `F(1)` for F1.
    F(u8),
    /// A character.
    Char(char),
    /// Null.
    Null,
    /// Escape.
    Esc,
    /// Caps Lock.
    CapsLock,
    /// Scroll Lock.
    ScrollLock,
    /// Num Lock.
    NumLock,
    /// Print Screen.
    PrintScreen,
    /// Pause.
    Pause,
    /// Menu.
    Menu,
    /// The keypad's 5 without Num Lock.
    KeypadBegin,
    /// A media key.
    Media(MediaKeyCode),
    /// A modifier key on its own.
    Modifier(ModifierKeyCode),
}

/// A key with the modifiers held down with it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, PartialOrd)]
pub struct KeyEvent {
    /// The key.
    pub code: KeyCode,
    /// The modifiers.
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    /// Create a key event.
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers }
    }
}

impl From<KeyCode> for KeyEvent {
    fn from(code: KeyCode) -> Self {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}

#[cfg(feature = "crossterm")]
mod from_crossterm {
    use super::*;
    use crossterm::event as ct;

    impl From<ct::KeyModifiers> for KeyModifiers {
        fn from(mods: ct::KeyModifiers) -> Self {
            KeyModifiers::from_bits_truncate(mods.bits())
        }
    }

    impl From<ct::MediaKeyCode> for MediaKeyCode {
        fn from(code: ct::MediaKeyCode) -> Self {
            match code {
                ct::MediaKeyCode::Play => MediaKeyCode::Play,
                ct::MediaKeyCode::Pause => MediaKeyCode::Pause,
                ct::MediaKeyCode::PlayPause => MediaKeyCode::PlayPause,
                ct::MediaKeyCode::Reverse => MediaKeyCode::Reverse,
                ct::MediaKeyCode::Stop => MediaKeyCode::Stop,
                ct::MediaKeyCode::FastForward => MediaKeyCode::FastForward,
                ct::MediaKeyCode::Rewind => MediaKeyCode::Rewind,
                ct::MediaKeyCode::TrackNext => MediaKeyCode::TrackNext,
                ct::MediaKeyCode::TrackPrevious => MediaKeyCode::TrackPrevious,
                ct::MediaKeyCode::Record => MediaKeyCode::Record,
                ct::MediaKeyCode::LowerVolume => MediaKeyCode::LowerVolume,
                ct::MediaKeyCode::RaiseVolume => MediaKeyCode::RaiseVolume,
                ct::MediaKeyCode::MuteVolume => MediaKeyCode::MuteVolume,
            }
        }
    }

    impl From<ct::ModifierKeyCode> for ModifierKeyCode {
        fn from(code: ct::ModifierKeyCode) -> Self {
            match code {
                ct::ModifierKeyCode::LeftShift => ModifierKeyCode::LeftShift,
                ct::ModifierKeyCode::LeftControl => ModifierKeyCode::LeftControl,
                ct::ModifierKeyCode::LeftAlt => ModifierKeyCode::LeftAlt,
                ct::ModifierKeyCode::LeftSuper => ModifierKeyCode::LeftSuper,
                ct::ModifierKeyCode::LeftHyper => ModifierKeyCode::LeftHyper,
                ct::ModifierKeyCode::LeftMeta => ModifierKeyCode::LeftMeta,
                ct::ModifierKeyCode::RightShift => ModifierKeyCode::RightShift,
                ct::ModifierKeyCode::RightControl => ModifierKeyCode::RightControl,
                ct::ModifierKeyCode::RightAlt => ModifierKeyCode::RightAlt,
                ct::ModifierKeyCode::RightSuper => ModifierKeyCode::RightSuper,
                ct::ModifierKeyCode::RightHyper => ModifierKeyCode::RightHyper,
                ct::ModifierKeyCode::RightMeta => ModifierKeyCode::RightMeta,
                ct::ModifierKeyCode::IsoLevel3Shift => ModifierKeyCode::IsoLevel3Shift,
                ct::ModifierKeyCode::IsoLevel5Shift => ModifierKeyCode::IsoLevel5Shift,
            }
        }
    }

    impl From<ct::KeyCode> for KeyCode {
        fn from(code: ct::KeyCode) -> Self {
            match code {
                ct::KeyCode::Backspace => KeyCode::Backspace,
                ct::KeyCode::Enter => KeyCode::Enter,
                ct::KeyCode::Left => KeyCode::Left,
                ct::KeyCode::Right => KeyCode::Right,
                ct::KeyCode::Up => KeyCode::Up,
                ct::KeyCode::Down => KeyCode::Down,
                ct::KeyCode::Home => KeyCode::Home,
                ct::KeyCode::End => KeyCode::End,
                ct::KeyCode::PageUp => KeyCode::PageUp,
                ct::KeyCode::PageDown => KeyCode::PageDown,
                ct::KeyCode::Tab => KeyCode::Tab,
                ct::KeyCode::BackTab => KeyCode::BackTab,
                ct::KeyCode::Delete => KeyCode::Delete,
                ct::KeyCode::Insert => KeyCode::Insert,
                ct::KeyCode::F(n) => KeyCode::F(n),
                ct::KeyCode::Char(c) => KeyCode::Char(c),
                ct::KeyCode::Null => KeyCode::Null,
                ct::KeyCode::Esc => KeyCode::Esc,
                ct::KeyCode::CapsLock => KeyCode::CapsLock,
                ct::KeyCode::ScrollLock => KeyCode::ScrollLock,
                ct::KeyCode::NumLock => KeyCode::NumLock,
                ct::KeyCode::PrintScreen => KeyCode::PrintScreen,
                ct::KeyCode::Pause => KeyCode::Pause,
                ct::KeyCode::Menu => KeyCode::Menu,
                ct::KeyCode::KeypadBegin => KeyCode::KeypadBegin,
                ct::KeyCode::Media(m) => KeyCode::Media(m.into()),
                ct::KeyCode::Modifier(m) => KeyCode::Modifier(m.into()),
            }
        }
    }

    impl From<ct::KeyEvent> for KeyEvent {
        fn from(ke: ct::KeyEvent) -> Self {
            KeyEvent::new(ke.code.into(), ke.modifiers.into())
        }
    }
}
