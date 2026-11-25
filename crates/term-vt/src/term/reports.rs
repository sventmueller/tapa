//! Reports and settings that the main table of control sequences hands on: the text area's size in characters,
//! DECRQM and DECST8C as `alacritty_terminal` answers them, DECRQCRA, which esctest reads the screen with, and the
//! soft reset DECSTR.

use super::{Event, Modes, Terminal, tab_stops};
use crate::parser::Params;

impl Terminal {
    /// The control sequences `Perform::csi` leaves to this table.
    pub(super) fn csi_report(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8) {
        match (action, private, intermediates) {
            (b'p', None, [b'$']) => self.report_mode(params.or(0, 0)),
            (b'p', Some(b'?'), [b'$']) => self.report_private_mode(params.or(0, 0)),
            (b'W', Some(b'?'), []) if params.or(0, 0) == 5 => self.tabs = tab_stops(self.size.columns),
            (b'y', None, [b'*']) => self.report_checksum(params),
            (b'p', None, [b'!']) => self.soft_reset(),
            _ => {}
        }
    }

    /// DECSTR, as xterm's soft reset: modes, margins, rendition, character sets and the saved cursor back to their
    /// defaults; the screen and the cursor's place stay.
    fn soft_reset(&mut self) {
        self.modes.insert(Modes::SHOW_CURSOR);
        self.modes.insert(Modes::LINE_WRAP);
        for mode in [Modes::INSERT, Modes::ORIGIN, Modes::APP_CURSOR, Modes::APP_KEYPAD] {
            self.modes.remove(mode);
        }
        self.region = 0..self.size.lines;
        self.cursor.style = crate::style::Style::default();
        self.cursor.charsets = [super::Charset::Ascii; 4];
        self.cursor.link = None;
        self.cursor.wrap_pending = false;
        self.active_charset = 0;
        self.cursor_style_id = None;
        self.saved = super::Cursor::default();
    }

    /// CSI 18 t: the text area's size in characters.
    pub(super) fn report_text_area_chars(&mut self) {
        let (lines, columns) = (self.size.lines, self.size.columns);
        self.answer(format!("\x1b[8;{lines};{columns}t"));
    }

    fn answer(&mut self, text: String) {
        self.events.push(Event::Reply(text.into_bytes()));
    }

    /// DECRQM for an ANSI mode: 1 set, 2 reset, 0 unknown.
    fn report_mode(&mut self, mode: u16) {
        let state = match mode {
            4 => state(self.modes.contains(Modes::INSERT)),
            20 => state(self.modes.contains(Modes::LINE_FEED_NEW_LINE)),
            _ => 0,
        };
        self.answer(format!("\x1b[{mode};{state}$y"));
    }

    /// DECRQM for a DEC private mode: 1 set, 2 reset, 0 unknown.
    fn report_private_mode(&mut self, mode: u16) {
        let flag = match mode {
            1 => Some(Modes::APP_CURSOR),
            6 => Some(Modes::ORIGIN),
            7 => Some(Modes::LINE_WRAP),
            25 => Some(Modes::SHOW_CURSOR),
            1000 => Some(Modes::MOUSE_CLICK),
            1002 => Some(Modes::MOUSE_DRAG),
            1003 => Some(Modes::MOUSE_MOTION),
            1004 => Some(Modes::FOCUS),
            1005 => Some(Modes::UTF8_MOUSE),
            1006 => Some(Modes::SGR_MOUSE),
            1007 => Some(Modes::ALTERNATE_SCROLL),
            1042 => Some(Modes::URGENCY),
            1049 => Some(Modes::ALT_SCREEN),
            2004 => Some(Modes::BRACKETED_PASTE),
            2026 => Some(Modes::SYNC),
            _ => None,
        };
        let state = match (mode, flag) {
            (_, Some(flag)) => state(self.modes.contains(flag)),
            (12, None) => state(self.cursor_shape.is_some_and(|(_, blinking)| blinking)),
            _ => 0,
        };
        self.answer(format!("\x1b[?{mode};{state}$y"));
    }

    /// DECRQCRA: `CSI Pid ; Pp ; Pt ; Pl ; Pb ; Pr * y`, the 16-bit sum of the characters in the rectangle, counted
    /// from the region's top in origin mode, with empty cells and the tab marks Alacritty leaves as blanks the way xterm counts them since patch 334;
    /// attributes do not count.
    fn report_checksum(&mut self, params: &Params) {
        let id = params.or(0, 0);
        let (lines, columns) = (self.size.lines, self.size.columns);
        let (offset, lines) = if self.modes.contains(Modes::ORIGIN) {
            (self.region.start, self.region.end)
        } else {
            (0, lines)
        };
        let top = offset + usize::from(params.or(2, 1)) - 1;
        let left = usize::from(params.or(3, 1)) - 1;
        let bottom = params.get(4).and_then(|p| p.first()).filter(|&&b| b != 0);
        let bottom = bottom.map_or(lines, |&b| (offset + usize::from(b)).min(lines));
        let right = usize::from(params.or(5, u16::try_from(columns).unwrap_or(u16::MAX))).min(columns);
        let mut sum: u16 = 0;
        for row in self.grid.rows.iter().take(bottom).skip(top) {
            for cell in row.cells.iter().take(right).skip(left) {
                let c = if matches!(cell.c, '\0' | '\t') { ' ' } else { cell.c };
                sum = sum.wrapping_add(u16::try_from(u32::from(c) & 0xffff).unwrap_or(0));
            }
        }
        self.answer(format!("\x1bP{id}!~{sum:04X}\x1b\\"));
    }
}

fn state(on: bool) -> u8 {
    if on { 1 } else { 2 }
}
