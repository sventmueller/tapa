//! UTF-8 decoding of printed text, byte by byte across reads; a malformed sequence prints U+FFFD, as xterm does.

/// A character being decoded.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Decoder {
    value: u32,
    /// Bytes still to come.
    left: u8,
    /// The smallest value the sequence may encode, to refuse overlong forms.
    least: u32,
}

impl Decoder {
    /// Whether a character is half read.
    pub(crate) fn pending(&self) -> bool {
        self.left > 0
    }

    /// Whether `byte` continues the character being read.
    pub(crate) fn continues(byte: u8) -> bool {
        byte & 0xc0 == 0x80
    }

    /// Takes `byte`, which continues a pending character or starts one; the character it completes, if any.
    pub(crate) fn push(&mut self, byte: u8) -> Option<char> {
        if self.left > 0 {
            self.value = (self.value << 6) | u32::from(byte & 0x3f);
            self.left -= 1;
            if self.left > 0 {
                return None;
            }
            let value = self.value;
            let least = self.least;
            *self = Decoder::default();
            if value < least {
                return Some(char::REPLACEMENT_CHARACTER);
            }
            return Some(char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
        match byte {
            0x00..=0x7f => Some(char::from(byte)),
            0xc2..=0xdf => self.start(byte & 0x1f, 1, 0x80),
            0xe0..=0xef => self.start(byte & 0x0f, 2, 0x800),
            0xf0..=0xf4 => self.start(byte & 0x07, 3, 0x1_0000),
            _ => Some(char::REPLACEMENT_CHARACTER),
        }
    }

    fn start(&mut self, bits: u8, left: u8, least: u32) -> Option<char> {
        self.value = u32::from(bits);
        self.left = left;
        self.least = least;
        None
    }
}
