//! The parser's actions, carried out on the terminal the way `vte`'s ANSI handler maps them for
//! `alacritty_terminal`, which the reference tests hold the core to (E11).

use std::sync::Arc;

use super::{Charset, Event, Modes, Terminal};
use crate::cell::Link;
use crate::parser::{Params, Perform};

impl Perform for Terminal {
    fn print(&mut self, c: char) {
        self.input(c);
        self.preceding = Some(c);
    }

    fn print_chars(&mut self, run: &[char]) {
        self.input_chars(run);
        if let Some(&last) = run.last() {
            self.preceding = Some(last);
        }
    }

    fn print_ascii(&mut self, run: &[u8]) {
        self.input_ascii(run);
        if let Some(&last) = run.last() {
            self.preceding = Some(char::from(last));
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x09 => self.put_tab(1),
            0x08 => self.backspace(),
            0x0d => self.carriage_return(),
            0x0a..=0x0c => {
                self.linefeed();
                if self.modes.contains(Modes::LINE_FEED_NEW_LINE) {
                    self.carriage_return();
                }
            }
            0x07 => self.events.push(Event::Bell),
            0x0f => self.active_charset = 0,
            0x0e => self.active_charset = 1,
            _ => {}
        }
    }

    fn csi(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8) {
        let first = |default: u16| params.or(0, default);
        let count = |default: u16| usize::from(params.or(0, default));
        match (action, private, intermediates) {
            (b'@', None, []) => self.insert_blank(count(1)),
            (b'A', None, []) => self.move_up(count(1)),
            (b'B', None, []) => self.move_down(count(1)),
            (b'e', None, []) => self.move_down_absolute(count(1)),
            (b'b', None, []) => self.repeat_preceding(first(1)),
            (b'C' | b'a', None, []) => self.move_forward(count(1)),
            (b'c', None | Some(b'>'), []) if params.or(0, 0) == 0 => self.identify(private.is_some()),
            (b'D', None, []) => self.move_backward(count(1)),
            (b'd', None, []) => self.goto_line(i64::from(first(1)) - 1),
            (b'E', None, []) => {
                self.move_down(count(1));
                self.carriage_return();
            }
            (b'F', None, []) => {
                self.move_up(count(1));
                self.carriage_return();
            }
            (b'G' | b'`', None, []) => self.goto_column(count(1) - 1),
            (b'g', None, []) => match params.or(0, 0) {
                0 => {
                    let column = self.cursor.column;
                    self.tabs[column] = false;
                }
                3 => self.tabs.iter_mut().for_each(|tab| *tab = false),
                _ => {}
            },
            (b'H' | b'f', None, []) => {
                let line = i64::from(params.or(0, 1)) - 1;
                let column = usize::from(params.or(1, 1)) - 1;
                self.goto(line, column);
            }
            (b'h', None, []) => params.iter().for_each(|p| self.set_mode(p[0], true)),
            (b'h', Some(b'?'), []) => params.iter().for_each(|p| self.set_private_mode(p[0], true)),
            (b'I', None, []) => self.move_forward_tabs(first(1)),
            (b'J', None, []) => self.clear_screen(params.or(0, 0)),
            (b'K', None, []) => self.clear_line(params.or(0, 0)),
            (b'L', None, []) => self.insert_blank_lines(count(1)),
            (b'l', None, []) => params.iter().for_each(|p| self.set_mode(p[0], false)),
            (b'l', Some(b'?'), []) => params.iter().for_each(|p| self.set_private_mode(p[0], false)),
            (b'M', None, []) => self.delete_lines(count(1)),
            (b'm', None, []) => {
                super::sgr::apply(&mut self.cursor.style, params);
                self.cursor_style_id = None;
            }
            (b'n', None, []) => self.device_status(params.or(0, 0)),
            (b'P', None, []) => self.delete_chars(count(1)),
            (b'q', None, [b' ']) => {
                self.cursor_shape = match params.or(0, 0) {
                    1 | 2 => Some((super::CursorShape::Block, params.or(0, 0) == 1)),
                    3 | 4 => Some((super::CursorShape::Underline, params.or(0, 0) == 3)),
                    5 | 6 => Some((super::CursorShape::Bar, params.or(0, 0) == 5)),
                    _ => None,
                };
            }
            (b'q', Some(b'>'), []) => self.report_version(),
            (b'r', None, []) => {
                let top = usize::from(params.or(0, 1));
                let bottom = params
                    .get(1)
                    .and_then(|p| p.first())
                    .map(|&b| usize::from(b))
                    .filter(|&b| b != 0);
                self.set_scrolling_region(top, bottom);
            }
            (b'S', None, []) => self.scroll_up(count(1)),
            (b's', None, []) => self.save_cursor(),
            (b'T', None, []) => self.scroll_down(count(1)),
            (b't', None, []) => match params.or(0, 0) {
                14 => self.events.push(Event::TextAreaSizeRequest),
                18 => self.report_text_area_chars(),
                22 => {
                    if self.title_stack.len() >= 4096 {
                        self.title_stack.remove(0);
                    }
                    self.title_stack.push(self.title.clone());
                }
                23 => {
                    if let Some(title) = self.title_stack.pop() {
                        self.title.clone_from(&title);
                        self.events.push(Event::Title(title));
                    }
                }
                _ => {}
            },
            (b'u', None, []) => self.restore_cursor(),
            (b'u', Some(b'?'), []) => self.reply(format!("\x1b[?{}u", self.keyboard_flags())),
            (b'u', Some(b'>'), []) => self.push_keyboard(params.or(0, 0)),
            (b'u', Some(b'<'), []) => self.pop_keyboard(count(1)),
            (b'u', Some(b'='), []) => self.set_keyboard(params.or(0, 0), params.or(1, 1)),
            (b'X', None, []) => self.erase_chars(count(1)),
            (b'Z', None, []) => self.move_backward_tabs(first(1)),
            _ => self.csi_report(params, intermediates, private, action),
        }
    }

    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        let charset = |intermediates: &[u8]| match intermediates {
            [b'('] => Some(0),
            [b')'] => Some(1),
            [b'*'] => Some(2),
            [b'+'] => Some(3),
            _ => None,
        };
        match (byte, intermediates) {
            (b'B', slot) => {
                if let Some(index) = charset(slot) {
                    self.cursor.charsets[index] = Charset::Ascii;
                }
            }
            (b'0', slot) => {
                if let Some(index) = charset(slot) {
                    self.cursor.charsets[index] = Charset::LineDrawing;
                }
            }
            (b'D', []) => self.linefeed(),
            (b'E', []) => {
                self.linefeed();
                self.carriage_return();
            }
            (b'H', []) => {
                let column = self.cursor.column;
                self.tabs[column] = true;
            }
            (b'M', []) => self.reverse_index(),
            (b'Z', []) => self.identify(false),
            (b'c', []) => self.reset(),
            (b'7', []) => self.save_cursor(),
            (b'8', [b'#']) => self.decaln(),
            (b'8', []) => self.restore_cursor(),
            (b'=', []) => self.modes.insert(Modes::APP_KEYPAD),
            (b'>', []) => self.modes.remove(Modes::APP_KEYPAD),
            _ => {}
        }
    }

    fn osc(&mut self, params: &[&[u8]], bell: bool) {
        let Some(&command) = params.first() else { return };
        let text = |parts: &[&[u8]]| {
            parts
                .iter()
                .filter_map(|part| std::str::from_utf8(part).ok())
                .collect::<Vec<_>>()
                .join(";")
        };
        match command {
            b"0" | b"2" if params.len() >= 2 => {
                let title = text(&params[1..]).trim().to_string();
                self.title = Some(title.clone());
                self.events.push(Event::Title(Some(title)));
            }
            b"8" if params.len() >= 3 => {
                let uri = text(&params[2..]);
                self.cursor.link = (!uri.is_empty()).then(|| {
                    let id = std::str::from_utf8(params[1])
                        .ok()
                        .and_then(|options| options.split(':').find_map(|option| option.strip_prefix("id=")))
                        .map(str::to_string);
                    Arc::new(Link { id, uri })
                });
            }
            b"133" if params.len() >= 2 => self.mark(&params[1..]),
            b"7" if params.len() >= 2 => self.set_directory(text(&params[1..]).as_bytes()),
            b"4" | b"10" | b"11" | b"12" if params.len() >= 2 => {
                let command = std::str::from_utf8(command)
                    .ok()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(4);
                self.set_colors(command, &params[1..], bell);
            }
            b"104" | b"110" | b"111" | b"112" => {
                let command = std::str::from_utf8(command)
                    .ok()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(104);
                self.reset_colors(command, &params[1..]);
            }
            b"52" if params.len() >= 3 && params[2] == b"?" => {
                if self.clipboard_read {
                    let clipboard = params[1].first().copied().unwrap_or(b'c');
                    let terminator = if bell { "\x07" } else { "\x1b\\" };
                    self.events.push(Event::ClipboardLoad(clipboard, terminator));
                }
            }
            b"52" if params.len() >= 3 => {
                let clipboard = params[1].first().copied().unwrap_or(b'c');
                if let Some(text) = base64(params[2]).and_then(|bytes| String::from_utf8(bytes).ok()) {
                    self.events.push(Event::ClipboardStore(clipboard, text));
                }
            }
            _ => {}
        }
    }

    fn dcs(&mut self, _params: &Params, _intermediates: &[u8], _private: Option<u8>, _action: u8, _data: &[u8]) {}
}

impl Terminal {
    fn reply(&mut self, text: String) {
        self.events.push(Event::Reply(text.into_bytes()));
    }

    /// DA1 and DA2: the terminal's identity, as `alacritty_terminal` gives it.
    fn identify(&mut self, secondary: bool) {
        if secondary {
            self.reply("\x1b[>0;1;1c".into());
        } else {
            self.reply("\x1b[?6c".into());
        }
    }

    /// XTVERSION: the product's name and version, which programs ask instead of trusting `TERM_PROGRAM` (E15).
    fn report_version(&mut self) {
        let identity = self.identity.clone();
        self.reply(format!("\x1bP>|{identity}\x1b\\"));
    }

    fn device_status(&mut self, which: u16) {
        match which {
            5 => self.reply("\x1b[0n".into()),
            6 => {
                let (line, column) = (self.cursor.line + 1, self.cursor.column + 1);
                self.reply(format!("\x1b[{line};{column}R"));
            }
            _ => {}
        }
    }

    fn set_mode(&mut self, mode: u16, on: bool) {
        let mode = match mode {
            4 => Modes::INSERT,
            20 => Modes::LINE_FEED_NEW_LINE,
            _ => return,
        };
        if on {
            self.modes.insert(mode);
        } else {
            self.modes.remove(mode);
        }
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        let flag = match mode {
            1 => Modes::APP_CURSOR,
            3 => return self.deccolm(),
            6 => {
                if on {
                    self.modes.insert(Modes::ORIGIN);
                } else {
                    self.modes.remove(Modes::ORIGIN);
                }
                self.goto(0, 0);
                return;
            }
            7 => Modes::LINE_WRAP,
            25 => Modes::SHOW_CURSOR,
            1000 | 1002 | 1003 => {
                self.modes.remove(Modes::MOUSE);
                if on {
                    self.modes.insert(match mode {
                        1000 => Modes::MOUSE_CLICK,
                        1002 => Modes::MOUSE_DRAG,
                        _ => Modes::MOUSE_MOTION,
                    });
                }
                return;
            }
            1004 => Modes::FOCUS,
            1005 => {
                self.modes.remove(Modes::SGR_MOUSE);
                Modes::UTF8_MOUSE
            }
            1006 => {
                self.modes.remove(Modes::UTF8_MOUSE);
                Modes::SGR_MOUSE
            }
            1007 => Modes::ALTERNATE_SCROLL,
            1042 => Modes::URGENCY,
            1049 => {
                if on != self.modes.contains(Modes::ALT_SCREEN) {
                    self.swap_alt();
                } else if on {
                    // xterm saves the cursor and clears the alternate screen again when it already shows.
                    self.save_cursor();
                    let blank = self.blank();
                    let lines = self.size.lines;
                    self.grid.reset_rows(0..lines, blank);
                    self.cursor.wrap_pending = false;
                } else {
                    // xterm restores the cursor on leaving even when the main screen already shows.
                    self.restore_cursor();
                }
                return;
            }
            2004 => Modes::BRACKETED_PASTE,
            2026 => Modes::SYNC,
            _ => return,
        };
        if on {
            self.modes.insert(flag);
        } else {
            self.modes.remove(flag);
        }
    }

    /// REP: the last printed character `count` times more.
    fn repeat_preceding(&mut self, count: u16) {
        if let Some(c) = self.preceding {
            for _ in 0..count {
                self.input(c);
            }
        }
    }

    fn keyboard_flags(&self) -> u16 {
        self.keyboard.last().copied().unwrap_or(0)
    }

    fn push_keyboard(&mut self, flags: u16) {
        if self.keyboard.len() >= 16 {
            self.keyboard.remove(0);
        }
        self.keyboard.push(flags);
    }

    fn pop_keyboard(&mut self, count: usize) {
        let keep = self.keyboard.len().saturating_sub(count);
        self.keyboard.truncate(keep);
    }

    fn set_keyboard(&mut self, flags: u16, how: u16) {
        let current = self.keyboard_flags();
        let flags = match how {
            2 => current | flags,
            3 => current & !flags,
            _ => flags,
        };
        match self.keyboard.last_mut() {
            Some(last) => *last = flags,
            None => self.keyboard.push(flags),
        }
    }
}

/// The bytes of standard base64 `text`, as OSC 52 carries them.
fn base64(text: &[u8]) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0;
    for &c in text.iter().filter(|&&c| c != b'=' && !c.is_ascii_whitespace()) {
        buffer = (buffer << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).unwrap_or(0));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_reads_with_and_without_padding() {
        assert_eq!(super::base64(b"aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(super::base64(b"aGk"), Some(b"hi".to_vec()));
        assert_eq!(super::base64(b"!!"), None);
    }
}
