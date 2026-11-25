//! The modes a program switches on and off.

/// A set of modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modes(pub u32);

#[expect(missing_docs, reason = "the names are the modes'")]
impl Modes {
    pub const SHOW_CURSOR: Modes = Modes(1);
    pub const APP_CURSOR: Modes = Modes(1 << 1);
    pub const APP_KEYPAD: Modes = Modes(1 << 2);
    pub const MOUSE_CLICK: Modes = Modes(1 << 3);
    pub const MOUSE_DRAG: Modes = Modes(1 << 4);
    pub const MOUSE_MOTION: Modes = Modes(1 << 5);
    pub const FOCUS: Modes = Modes(1 << 6);
    pub const BRACKETED_PASTE: Modes = Modes(1 << 7);
    pub const SGR_MOUSE: Modes = Modes(1 << 8);
    pub const UTF8_MOUSE: Modes = Modes(1 << 9);
    pub const ALTERNATE_SCROLL: Modes = Modes(1 << 10);
    pub const LINE_WRAP: Modes = Modes(1 << 11);
    pub const ORIGIN: Modes = Modes(1 << 12);
    pub const INSERT: Modes = Modes(1 << 13);
    pub const LINE_FEED_NEW_LINE: Modes = Modes(1 << 14);
    pub const ALT_SCREEN: Modes = Modes(1 << 15);
    pub const URGENCY: Modes = Modes(1 << 16);
    pub const SYNC: Modes = Modes(1 << 17);
    /// Every mouse reporting mode, of which one is on at most.
    pub const MOUSE: Modes = Modes((1 << 3) | (1 << 4) | (1 << 5));
}

impl Default for Modes {
    fn default() -> Modes {
        Modes(Modes::SHOW_CURSOR.0 | Modes::LINE_WRAP.0 | Modes::ALTERNATE_SCROLL.0)
    }
}

impl Modes {
    /// Whether all of `other` are on.
    pub fn contains(self, other: Modes) -> bool {
        self.0 & other.0 == other.0
    }

    /// Switches `other` on.
    pub fn insert(&mut self, other: Modes) {
        self.0 |= other.0;
    }

    /// Switches `other` off.
    pub fn remove(&mut self, other: Modes) {
        self.0 &= !other.0;
    }
}
