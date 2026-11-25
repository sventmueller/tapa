//! Keys, mouse, paste and focus as the bytes a terminal program reads, independent of AppKit.

/// The modes a program has switched on that change what input looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputModes {
    /// Cursor keys send `ESC O` instead of `ESC [` (DECCKM).
    pub(crate) app_cursor: bool,
    /// The keypad sends its own sequences (DECKPAM).
    pub(crate) app_keypad: bool,
    /// Enter sends CR LF (LNM).
    pub(crate) newline: bool,
    /// Pasted text is wrapped in `ESC [200~` and `ESC [201~`.
    pub bracketed_paste: bool,
    /// Focus changes are reported as `ESC [I` and `ESC [O`.
    pub focus_reporting: bool,
    /// Which mouse events the program wants.
    pub mouse: MouseTracking,
    /// How mouse events are written.
    pub(crate) mouse_encoding: MouseEncoding,
    /// The alternate screen is shown, as in full-screen programs.
    pub alternate_screen: bool,
    /// On the alternate screen without mouse reporting, the wheel sends cursor keys.
    pub(crate) alternate_scroll: bool,
    /// The kitty keyboard protocol's progressive enhancements the program asked for.
    pub(crate) kitty: KittyFlags,
}

/// The kitty keyboard protocol's progressive enhancements, as the program pushed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub(crate) struct KittyFlags(pub(crate) u8);

impl KittyFlags {
    /// Legacy encoding.
    pub(crate) const NONE: KittyFlags = KittyFlags(0);
    /// Keys that are ambiguous in the legacy encoding, such as Esc or Ctrl+I, become `CSI u` sequences.
    pub(crate) const DISAMBIGUATE: KittyFlags = KittyFlags(1);
    /// Repeats and releases are reported too.
    pub(crate) const EVENT_TYPES: KittyFlags = KittyFlags(2);
    /// The shifted key is reported next to the base key.
    pub(crate) const ALTERNATE_KEYS: KittyFlags = KittyFlags(4);
    /// Every key, text keys included, becomes an escape sequence.
    pub(crate) const ALL_KEYS: KittyFlags = KittyFlags(8);
    /// The text a key types is reported with its sequence.
    pub(crate) const ASSOCIATED_TEXT: KittyFlags = KittyFlags(16);

    /// Whether all of `flags` are set.
    pub(crate) fn contains(self, flags: KittyFlags) -> bool {
        self.0 & flags.0 == flags.0
    }

    /// These flags and `flags`.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn with(self, flags: KittyFlags) -> KittyFlags {
        KittyFlags(self.0 | flags.0)
    }

    /// Whether no flag is set.
    #[cfg(test)]
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The flags as the protocol numbers them.
    #[cfg(test)]
    pub(crate) fn bits(self) -> u8 {
        self.0
    }
}

/// Which mouse events a program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseTracking {
    /// None; the terminal selects text.
    #[default]
    Off,
    /// Presses and releases (mode 1000).
    Clicks,
    /// Also movement while a button is held (mode 1002).
    ButtonMotion,
    /// Also every movement (mode 1003).
    AnyMotion,
}

/// How mouse events are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MouseEncoding {
    /// Three bytes after `CSI M`, up to column and line 223.
    #[default]
    Default,
    /// The same with coordinates as UTF-8 characters, up to 2015 (mode 1005).
    Utf8,
    /// Decimal numbers, any size, with the release told apart (mode 1006).
    Sgr,
}

/// Modifier keys held during an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers {
    /// ⇧.
    pub shift: bool,
    /// ⌃.
    pub control: bool,
    /// ⌥ when it acts as Alt/Meta; when it types characters, the view leaves it out and passes the text.
    pub alt: bool,
    /// ⌘, which reaches programs only through the kitty protocol.
    pub command: bool,
}

impl Modifiers {
    fn is_empty(self) -> bool {
        !(self.shift || self.control || self.alt || self.command)
    }

    fn only_shift(self) -> bool {
        self.shift && !(self.control || self.alt || self.command)
    }

    /// xterm's modifier parameter: 1 plus Shift 1, Alt 2, Ctrl 4.
    fn xterm(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.control)
    }

    /// The kitty protocol's modifier parameter, which adds Super 8 for ⌘.
    fn kitty(self) -> u8 {
        self.xterm() + 8 * u8::from(self.command)
    }
}

/// A key event.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Key {
    pub code: KeyCode,
    /// The modifiers held, including the key itself when it is a modifier being pressed.
    pub modifiers: Modifiers,
    /// What the key types with Shift (and ⌥ when it does not act as Alt) applied, such as `A` or `!`; `None` for
    /// keys that type nothing. Control characters are derived here and do not belong in it.
    pub text: Option<String>,
    /// The key is on the numeric keypad.
    pub keypad: bool,
    /// Press, repeat or release.
    pub action: KeyAction,
}

impl Key {
    /// A press of a key that types `text`, such as `Key::text('a')`.
    pub fn text(c: char) -> Key {
        Key {
            code: KeyCode::Char(c.to_lowercase().next().unwrap_or(c)),
            modifiers: Modifiers::default(),
            text: Some(c.to_string()),
            keypad: false,
            action: KeyAction::Press,
        }
    }

    /// A press of `key`.
    pub fn named(key: NamedKey) -> Key {
        Key {
            code: KeyCode::Named(key),
            modifiers: Modifiers::default(),
            text: None,
            keypad: false,
            action: KeyAction::Press,
        }
    }

    /// The key with `modifiers` held.
    #[must_use]
    pub fn with(mut self, modifiers: Modifiers) -> Key {
        self.modifiers = modifiers;
        self
    }
}

/// A key, by what it types or by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// A key that types a character: the character without modifiers, such as `a` for ⇧A or `1` for ⇧1 on a US
    /// layout.
    Char(char),
    /// A key that types nothing or a control character.
    Named(NamedKey),
}

/// Keys by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names are the keys'")]
pub enum NamedKey {
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// Forward delete (⌦).
    Delete,
    /// F1 to F35.
    F(u8),
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,
    Menu,
    /// A modifier key itself, reported only with the kitty protocol's `ALL_KEYS`.
    Modifier(ModifierKey, Side),
}

/// A modifier key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names are the keys'")]
pub enum ModifierKey {
    Shift,
    Control,
    Alt,
    Command,
}

/// Which of a pair of keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names say it")]
pub enum Side {
    Left,
    Right,
}

/// What happened to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum KeyAction {
    /// It went down.
    #[default]
    Press,
    /// It is held and repeats.
    Repeat,
    /// It came up; only programs using the kitty protocol hear of it.
    Release,
}

/// The bytes `key` sends to a program in `modes`; empty when it sends nothing.
pub(crate) fn encode_key(key: &Key, modes: &InputModes) -> Vec<u8> {
    let flags = modes.kitty;
    let kitty = flags.contains(KittyFlags::DISAMBIGUATE)
        || flags.contains(KittyFlags::EVENT_TYPES)
        || flags.contains(KittyFlags::ALL_KEYS);
    if !kitty {
        return legacy(key, modes);
    }
    if key.action == KeyAction::Release {
        let plain_release = matches!(
            key.code,
            KeyCode::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace)
        );
        if !flags.contains(KittyFlags::EVENT_TYPES) || (plain_release && !flags.contains(KittyFlags::ALL_KEYS)) {
            return Vec::new();
        }
        return kitty_sequence(key, flags).unwrap_or_default();
    }
    if needs_kitty_sequence(key, modes) {
        kitty_sequence(key, flags).unwrap_or_default()
    } else {
        legacy(key, modes)
    }
}

/// Whether the kitty protocol encodes `key` as a sequence rather than as the text or legacy bytes it types.
fn needs_kitty_sequence(key: &Key, modes: &InputModes) -> bool {
    let flags = modes.kitty;
    if flags.contains(KittyFlags::ALL_KEYS) {
        return true;
    }
    let plain_named = matches!(
        key.code,
        KeyCode::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace)
    );
    let disambiguate = flags.contains(KittyFlags::DISAMBIGUATE)
        && (key.code == KeyCode::Named(NamedKey::Escape)
            || key.keypad
            || (!key.modifiers.is_empty() && (!key.modifiers.only_shift() || plain_named)));
    if disambiguate {
        return true;
    }
    match key.code {
        KeyCode::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace | NamedKey::Escape) => false,
        KeyCode::Named(_) => true,
        KeyCode::Char(_) => legacy(key, modes).is_empty(),
    }
}

/// The kitty protocol's `CSI code[:alternate] ; modifiers[:event] ; text u` and its legacy-shaped forms.
fn kitty_sequence(key: &Key, flags: KittyFlags) -> Option<Vec<u8>> {
    let event = flags.contains(KittyFlags::EVENT_TYPES) && key.action != KeyAction::Press;
    let text = key
        .text
        .as_deref()
        .filter(|text| {
            flags.contains(KittyFlags::ASSOCIATED_TEXT)
                && key.action != KeyAction::Release
                && !key.modifiers.control
                && !key.modifiers.command
                && !text.is_empty()
                && !text.chars().any(char::is_control)
        })
        .map(str::to_string);
    let modifiers = key.modifiers.kitty();
    let detailed = event || modifiers > 1 || text.is_some();
    let (code, terminator) = kitty_code(key, flags, detailed)?;
    let mut sequence = format!("\x1b[{code}");
    if detailed {
        sequence.push_str(&format!(";{modifiers}"));
    }
    if event {
        sequence.push_str(if key.action == KeyAction::Repeat { ":2" } else { ":3" });
    }
    if let Some(text) = text {
        let codepoints: Vec<String> = text.chars().map(|c| u32::from(c).to_string()).collect();
        sequence.push(';');
        sequence.push_str(&codepoints.join(":"));
    }
    sequence.push(terminator);
    Some(sequence.into_bytes())
}

/// The number and final character of a key in the kitty protocol; arrows, Home, End and F1–F4 keep their legacy
/// finals with the number 1, which is left out when nothing follows.
fn kitty_code(key: &Key, flags: KittyFlags, detailed: bool) -> Option<(String, char)> {
    if key.keypad
        && let Some(code) = keypad_code(key.code)
    {
        return Some((code.to_string(), 'u'));
    }
    let one = if detailed { "1" } else { "" };
    let named = match key.code {
        KeyCode::Char(c) => return Some((char_code(key, c, flags), 'u')),
        KeyCode::Named(named) => named,
    };
    let legacy_final = |end: char| Some((one.to_string(), end));
    match named {
        NamedKey::Up => legacy_final('A'),
        NamedKey::Down => legacy_final('B'),
        NamedKey::Right => legacy_final('C'),
        NamedKey::Left => legacy_final('D'),
        NamedKey::Home => legacy_final('H'),
        NamedKey::End => legacy_final('F'),
        NamedKey::F(1) => legacy_final('P'),
        NamedKey::F(2) => legacy_final('Q'),
        NamedKey::F(4) => legacy_final('S'),
        NamedKey::F(n @ 13..=35) => Some(((57376 + u32::from(n) - 13).to_string(), 'u')),
        NamedKey::Modifier(_, _) | NamedKey::CapsLock | NamedKey::NumLock if !flags.contains(KittyFlags::ALL_KEYS) => {
            None
        }
        named => {
            let (code, end) = numbered(named)?;
            Some((code.to_string(), end))
        }
    }
}

/// Keys that are a number and a final in the kitty protocol.
fn numbered(named: NamedKey) -> Option<(u32, char)> {
    Some(match named {
        NamedKey::Enter => (13, 'u'),
        NamedKey::Tab => (9, 'u'),
        NamedKey::Backspace => (127, 'u'),
        NamedKey::Escape => (27, 'u'),
        NamedKey::Insert => (2, '~'),
        NamedKey::Delete => (3, '~'),
        NamedKey::PageUp => (5, '~'),
        NamedKey::PageDown => (6, '~'),
        // CSI R would read as a cursor position report, so F3 has a number of its own.
        NamedKey::F(3) => (13, '~'),
        NamedKey::F(n @ 5..=12) => (u32::from(function_number(n)?), '~'),
        NamedKey::CapsLock => (57358, 'u'),
        NamedKey::ScrollLock => (57359, 'u'),
        NamedKey::NumLock => (57360, 'u'),
        NamedKey::PrintScreen => (57361, 'u'),
        NamedKey::Pause => (57362, 'u'),
        NamedKey::Menu => (57363, 'u'),
        NamedKey::Modifier(modifier, side) => (modifier_code(modifier, side), 'u'),
        _ => return None,
    })
}

fn modifier_code(modifier: ModifierKey, side: Side) -> u32 {
    let left = match modifier {
        ModifierKey::Shift => 57441,
        ModifierKey::Control => 57442,
        ModifierKey::Alt => 57443,
        ModifierKey::Command => 57444,
    };
    if side == Side::Right { left + 6 } else { left }
}

fn keypad_code(code: KeyCode) -> Option<u32> {
    Some(match code {
        KeyCode::Char(digit @ '0'..='9') => 57399 + (u32::from(digit) - u32::from('0')),
        KeyCode::Char('.') => 57409,
        KeyCode::Char('/') => 57410,
        KeyCode::Char('*') => 57411,
        KeyCode::Char('-') => 57412,
        KeyCode::Char('+') => 57413,
        KeyCode::Named(NamedKey::Enter) => 57414,
        KeyCode::Char('=') => 57415,
        KeyCode::Char(',') => 57416,
        KeyCode::Named(NamedKey::Left) => 57417,
        KeyCode::Named(NamedKey::Right) => 57418,
        KeyCode::Named(NamedKey::Up) => 57419,
        KeyCode::Named(NamedKey::Down) => 57420,
        KeyCode::Named(NamedKey::PageUp) => 57421,
        KeyCode::Named(NamedKey::PageDown) => 57422,
        KeyCode::Named(NamedKey::Home) => 57423,
        KeyCode::Named(NamedKey::End) => 57424,
        KeyCode::Named(NamedKey::Insert) => 57425,
        KeyCode::Named(NamedKey::Delete) => 57426,
        _ => return None,
    })
}

/// A character key's code: the unshifted key, and with `ALTERNATE_KEYS` the shifted one after a colon.
fn char_code(key: &Key, c: char, flags: KittyFlags) -> String {
    let base = if key.modifiers.shift {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    };
    let shifted = key
        .text
        .as_deref()
        .filter(|_| key.modifiers.shift)
        .and_then(|text| {
            let mut chars = text.chars();
            chars.next().filter(|_| chars.next().is_none())
        })
        .filter(|&shifted| shifted != base);
    match shifted {
        Some(shifted) if flags.contains(KittyFlags::ALTERNATE_KEYS) => {
            format!("{}:{}", u32::from(base), u32::from(shifted))
        }
        _ => u32::from(base).to_string(),
    }
}

/// The xterm number of F5 to F20, which skips some values.
fn function_number(n: u8) -> Option<u8> {
    Some(match n {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        _ => return None,
    })
}

/// xterm's encoding, which every terminal program understands.
fn legacy(key: &Key, modes: &InputModes) -> Vec<u8> {
    if key.action == KeyAction::Release || key.modifiers.command {
        return Vec::new();
    }
    match key.code {
        KeyCode::Named(named) => legacy_named(named, key.keypad, key.modifiers, modes),
        KeyCode::Char(c) => legacy_char(key, c, modes),
    }
}

fn legacy_char(key: &Key, c: char, modes: &InputModes) -> Vec<u8> {
    let modifiers = key.modifiers;
    if key.keypad
        && modes.app_keypad
        && modifiers.is_empty()
        && let Some(final_byte) = keypad_final(c)
    {
        return vec![0x1b, b'O', final_byte];
    }
    let mut bytes = Vec::new();
    if modifiers.alt {
        bytes.push(0x1b);
    }
    if modifiers.control
        && let Some(control) = control_byte(c)
    {
        bytes.push(control);
        return bytes;
    }
    match key.text.as_deref() {
        Some(text) => bytes.extend_from_slice(text.as_bytes()),
        None => return Vec::new(),
    }
    bytes
}

/// The byte Ctrl and a key send, as xterm maps them.
fn control_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        letter @ 'a'..='z' => u8::try_from(u32::from(letter) - u32::from('a') + 1).ok()?,
        '@' | ' ' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '-' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// The final of a keypad key's `ESC O` sequence in application keypad mode.
fn keypad_final(c: char) -> Option<u8> {
    Some(match c {
        digit @ '0'..='9' => b'p' + u8::try_from(u32::from(digit) - u32::from('0')).ok()?,
        '.' => b'n',
        '+' => b'k',
        '-' => b'm',
        '*' => b'j',
        '/' => b'o',
        '=' => b'X',
        ',' => b'l',
        _ => return None,
    })
}

fn legacy_named(named: NamedKey, keypad: bool, modifiers: Modifiers, modes: &InputModes) -> Vec<u8> {
    let alt = |bytes: &[u8]| {
        let mut out = if modifiers.alt { vec![0x1b] } else { Vec::new() };
        out.extend_from_slice(bytes);
        out
    };
    let m = modifiers.xterm();
    let cursor = |end: char| {
        if m > 1 {
            format!("\x1b[1;{m}{end}").into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let tilde = |number: u8| {
        if m > 1 {
            format!("\x1b[{number};{m}~").into_bytes()
        } else {
            format!("\x1b[{number}~").into_bytes()
        }
    };
    match named {
        NamedKey::Enter if keypad && modes.app_keypad => b"\x1bOM".to_vec(),
        NamedKey::Enter if modes.newline => alt(b"\r\n"),
        NamedKey::Enter => alt(b"\r"),
        NamedKey::Tab if modifiers.shift => b"\x1b[Z".to_vec(),
        NamedKey::Tab => alt(b"\t"),
        NamedKey::Backspace if modifiers.control => alt(b"\x08"),
        NamedKey::Backspace => alt(b"\x7f"),
        NamedKey::Escape => alt(b"\x1b"),
        NamedKey::Up => cursor('A'),
        NamedKey::Down => cursor('B'),
        NamedKey::Right => cursor('C'),
        NamedKey::Left => cursor('D'),
        NamedKey::Home => cursor('H'),
        NamedKey::End => cursor('F'),
        NamedKey::Insert => tilde(2),
        NamedKey::Delete => tilde(3),
        NamedKey::PageUp => tilde(5),
        NamedKey::PageDown => tilde(6),
        NamedKey::F(n @ 1..=4) => {
            let end = char::from(b'P' + (n - 1));
            if m > 1 {
                format!("\x1b[1;{m}{end}").into_bytes()
            } else {
                format!("\x1bO{end}").into_bytes()
            }
        }
        NamedKey::F(n) => function_number(n).map(tilde).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// A mouse button, wheel direction included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names say it")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

impl MouseButton {
    fn code(self) -> u8 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::WheelUp => 64,
            MouseButton::WheelDown => 65,
            MouseButton::WheelLeft => 66,
            MouseButton::WheelRight => 67,
            MouseButton::Back => 128,
            MouseButton::Forward => 129,
        }
    }

    fn is_wheel(self) -> bool {
        self.code() & 64 != 0 && self.code() < 128
    }
}

/// What the mouse did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseAction {
    /// A button went down; wheel steps are presses.
    Press(MouseButton),
    /// A button came up.
    Release(MouseButton),
    /// The pointer moved into another cell, with the button held if any.
    Move(Option<MouseButton>),
}

/// A mouse event in a cell of the visible screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    /// What happened.
    pub action: MouseAction,
    /// The line on screen, from 0 at the top.
    pub line: usize,
    /// The column, from 0 at the left.
    pub column: usize,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

/// The report of `event` for a program in `modes`; `None` when it did not ask for this kind of event or the cell is
/// beyond what the encoding can express.
pub(crate) fn encode_mouse(event: &MouseEvent, modes: &InputModes) -> Option<Vec<u8>> {
    let (button, released, moved) = match event.action {
        MouseAction::Press(button) => (button.code(), false, false),
        MouseAction::Release(button) if button.is_wheel() => return None,
        MouseAction::Release(button) => (button.code(), true, false),
        MouseAction::Move(held) => {
            let wanted = match modes.mouse {
                MouseTracking::AnyMotion => true,
                MouseTracking::ButtonMotion => held.is_some(),
                MouseTracking::Clicks | MouseTracking::Off => false,
            };
            if !wanted {
                return None;
            }
            (held.map_or(3, MouseButton::code), false, true)
        }
    };
    if modes.mouse == MouseTracking::Off {
        return None;
    }
    let modifiers = 4 * u8::from(event.modifiers.shift)
        + 8 * u8::from(event.modifiers.alt)
        + 16 * u8::from(event.modifiers.control);
    let code = button + modifiers + if moved { 32 } else { 0 };
    if modes.mouse_encoding == MouseEncoding::Sgr {
        let end = if released { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", event.column + 1, event.line + 1).into_bytes());
    }
    // Before SGR there was no way to say which button came up.
    let code = if released { 3 + modifiers } else { code };
    let utf8 = modes.mouse_encoding == MouseEncoding::Utf8;
    let mut report = vec![0x1b, b'[', b'M', 32 + code];
    push_coordinate(&mut report, event.column, utf8)?;
    push_coordinate(&mut report, event.line, utf8)?;
    Some(report)
}

fn push_coordinate(report: &mut Vec<u8>, position: usize, utf8: bool) -> Option<()> {
    let value = u32::try_from(position).ok()?.checked_add(33)?;
    if utf8 {
        let c = char::from_u32(value).filter(|_| value < 2048)?;
        let mut buffer = [0; 4];
        report.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
    } else {
        report.push(u8::try_from(value).ok()?);
    }
    Some(())
}

/// What a turn of the wheel does in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wheel {
    /// Send these bytes: mouse reports, or cursor keys on the alternate screen.
    Send(Vec<u8>),
    /// Scroll the history by this many lines, positive towards older output.
    Scroll(i32),
}

/// What turning the wheel by `lines` (positive away from the user) at a cell does in `modes`. Shift scrolls the
/// history even on the alternate screen.
pub(crate) fn encode_wheel(lines: i32, line: usize, column: usize, modifiers: Modifiers, modes: &InputModes) -> Wheel {
    if lines == 0 {
        return Wheel::Send(Vec::new());
    }
    let button = if lines > 0 {
        MouseButton::WheelUp
    } else {
        MouseButton::WheelDown
    };
    let steps = lines.unsigned_abs().min(100);
    if modes.mouse != MouseTracking::Off {
        let event = MouseEvent {
            action: MouseAction::Press(button),
            line,
            column,
            modifiers,
        };
        let report = encode_mouse(&event, modes).unwrap_or_default();
        return Wheel::Send((0..steps).flat_map(|_| report.iter().copied()).collect());
    }
    if modes.alternate_screen && modes.alternate_scroll && !modifiers.shift {
        let arrow = if lines > 0 { NamedKey::Up } else { NamedKey::Down };
        let bytes = encode_key(
            &Key::named(arrow),
            &InputModes {
                kitty: KittyFlags::NONE,
                ..*modes
            },
        );
        return Wheel::Send((0..steps).flat_map(|_| bytes.iter().copied()).collect());
    }
    Wheel::Scroll(lines)
}

/// The report of a focus change, if the program asked for them.
pub(crate) fn encode_focus(focused: bool, modes: &InputModes) -> Option<&'static [u8]> {
    modes
        .focus_reporting
        .then_some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}

/// Pasted text as the program should receive it: inside bracketed-paste markers when it asked for them, without any
/// end marker of its own that would let the rest run as typed input; otherwise with line breaks as Enter sends them.
pub(crate) fn encode_paste(text: &str, modes: &InputModes) -> Vec<u8> {
    if !modes.bracketed_paste {
        return text.replace("\r\n", "\r").replace('\n', "\r").into_bytes();
    }
    let end = PASTE_END.as_bytes();
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    let inner = bytes.len();
    // A marker that removing another one joins together ends at the `~` that completes it, and the marker overlaps
    // no copy of itself, so this one pass leaves what removing markers until none is left would.
    for piece in text.as_bytes().split_inclusive(|&byte| byte == b'~') {
        bytes.extend_from_slice(piece);
        if bytes.len() - inner >= end.len() && bytes.ends_with(end) {
            bytes.truncate(bytes.len() - end.len());
        }
    }
    bytes.extend_from_slice(end);
    bytes
}

const PASTE_END: &str = "\x1b[201~";

#[cfg(test)]
mod tests;
