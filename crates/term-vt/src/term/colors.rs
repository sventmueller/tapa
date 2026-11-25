//! Colours a program changes or asks for (OSC 4, 10, 11, 12 and their resets 104, 110, 111, 112): the changes are
//! kept here and applied when a frame is drawn; questions are answered with the change or the theme's colour.

use super::{Event, Terminal};

/// Where the default text colour, background and cursor sit after the 256 colours of the table.
pub const FOREGROUND: usize = 256;
/// The background's place.
pub const BACKGROUND: usize = 257;
/// The cursor's place.
pub const CURSOR: usize = 258;
/// How many colours a program can change.
pub const COUNT: usize = 259;

impl Terminal {
    /// OSC 4 (`index;spec` pairs), or OSC 10 to 12 (`spec`s for text, background and cursor in turn).
    pub(super) fn set_colors(&mut self, command: u16, parts: &[&[u8]], bell: bool) {
        let terminator = if bell { "\x07" } else { "\x1b\\" };
        if command == 4 {
            for pair in parts.chunks(2) {
                let Some(index) = pair
                    .first()
                    .and_then(|i| std::str::from_utf8(i).ok())
                    .and_then(|i| i.parse::<u8>().ok())
                else {
                    continue;
                };
                let index = usize::from(index);
                let Some(&spec) = pair.get(1) else { continue };
                self.set_color(index, spec, &format!("4;{index}"), terminator);
            }
            return;
        }
        for (offset, &spec) in parts.iter().enumerate() {
            let slot = usize::from(command - 10) + offset;
            let Some(index) = [FOREGROUND, BACKGROUND, CURSOR].get(slot) else {
                break;
            };
            self.set_color(*index, spec, &(10 + slot).to_string(), terminator);
        }
    }

    fn set_color(&mut self, index: usize, spec: &[u8], prefix: &str, terminator: &str) {
        if index >= COUNT {
            return;
        }
        if spec == b"?" {
            let (r, g, b) = self.colors[index].unwrap_or(self.base_colors[index]);
            self.events.push(Event::Reply(
                format!("\x1b]{prefix};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}{terminator}").into_bytes(),
            ));
            return;
        }
        if let Some(rgb) = parse(spec) {
            self.colors[index] = Some(rgb);
        }
    }

    /// OSC 104 and 110 to 112: the colours back to the theme's; 104 without indices resets the whole table.
    pub(super) fn reset_colors(&mut self, command: u16, parts: &[&[u8]]) {
        match command {
            104 if parts.iter().all(|part| part.is_empty()) => self.colors[..256].fill(None),
            104 => {
                for part in parts {
                    if let Some(index) = std::str::from_utf8(part).ok().and_then(|i| i.parse::<usize>().ok())
                        && index < 256
                    {
                        self.colors[index] = None;
                    }
                }
            }
            110 => self.colors[FOREGROUND] = None,
            111 => self.colors[BACKGROUND] = None,
            112 => self.colors[CURSOR] = None,
            _ => {}
        }
    }

    /// The colours the program changed, by index; `None` where the theme's apply.
    pub fn color_changes(&self) -> &[Option<(u8, u8, u8)>] {
        &self.colors
    }

    /// The theme's colours, which answer questions about colours the program did not change.
    pub fn set_base_colors(&mut self, colors: &[(u8, u8, u8)]) {
        for (slot, color) in self.base_colors.iter_mut().zip(colors) {
            *slot = *color;
        }
    }
}

/// A colour as `rgb:rr/gg/bb` (one to four hex digits each) or `#rgb`, `#rrggbb` and longer.
fn parse(spec: &[u8]) -> Option<(u8, u8, u8)> {
    if let Some(hex) = spec.strip_prefix(b"#") {
        let third = hex.len() / 3;
        let (r, rest) = hex.split_at(third);
        let (g, b) = rest.split_at(third);
        return Some((legacy(r)?, legacy(g)?, legacy(b)?));
    }
    let text = std::str::from_utf8(spec).ok()?;
    let channels: Vec<&str> = text.strip_prefix("rgb:")?.split('/').collect();
    let [r, g, b] = channels.as_slice() else {
        return None;
    };
    let scale = |part: &str| -> Option<u8> {
        if part.is_empty() || part.len() > 4 {
            return None;
        }
        let value = u32::from_str_radix(part, 16).ok()?;
        let most = (1u32 << (4 * part.len())) - 1;
        u8::try_from(value * 255 / most).ok()
    };
    Some((scale(r)?, scale(g)?, scale(b)?))
}

/// One channel of the `#` form: its first two hex digits, a single digit standing for the high one, as `vte` reads it.
fn legacy(part: &[u8]) -> Option<u8> {
    if part.is_empty() || part.len() > 15 {
        return None;
    }
    let value = u64::from_str_radix(std::str::from_utf8(part).ok()?, 16).ok()? << 4;
    u8::try_from(value >> (4 * (part.len() - 1))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Size;

    #[test]
    fn colours_change_answer_and_reset() {
        let mut terminal = Terminal::new(Size { columns: 4, lines: 2 }, 0);
        terminal.advance(b"\x1b]4;1;rgb:ff/00/00\x07\x1b]11;#102030\x1b\\\x1b]4;1;?\x07");
        assert_eq!(terminal.color_changes()[1], Some((255, 0, 0)));
        assert_eq!(terminal.color_changes()[BACKGROUND], Some((0x10, 0x20, 0x30)));
        assert!(
            terminal
                .take_events()
                .contains(&Event::Reply(b"\x1b]4;1;rgb:ffff/0000/0000\x07".to_vec()))
        );
        terminal.advance(b"\x1b]104\x07\x1b]111\x07");
        assert_eq!(terminal.color_changes()[1], None);
        assert_eq!(terminal.color_changes()[BACKGROUND], None);
        assert_eq!(parse(b"rgb:f/8/0"), Some((255, 136, 0)));
    }
}
