//! The parser of the VT core (E4): Paul Williams' DEC ANSI state machine as xterm implements it, with UTF-8 decoding
//! and a fast path that hands runs of printable ASCII over in one call, found with `memchr` instead of byte by byte.

mod utf8;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod vte_tests;

/// The most parameters a sequence keeps; the rest are dropped, as xterm drops them.
pub const MAX_PARAMS: usize = 32;

/// The most bytes of an OSC string the parser keeps.
const MAX_OSC: usize = 64 * 1024;

/// The most bytes of a DCS string's data the parser keeps.
const MAX_DCS: usize = 1024 * 1024;

/// What a parsed stream does; the terminal implements it.
pub trait Perform {
    /// Prints `c`.
    fn print(&mut self, c: char);
    /// Prints a run of printable ASCII, which the terminal can place in one go.
    fn print_ascii(&mut self, run: &[u8]) {
        for &byte in run {
            self.print(char::from(byte));
        }
    }
    /// Prints a run of characters beyond ASCII, which the terminal can place with one look at its modes.
    fn print_chars(&mut self, run: &[char]) {
        for &c in run {
            self.print(c);
        }
    }
    /// Carries out the C0 or C1 control `byte`.
    fn execute(&mut self, byte: u8);
    /// A control sequence: `CSI` `intermediates` `params` `action`; `private` is the marker before the parameters,
    /// such as `?` or `>`.
    fn csi(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8);
    /// An escape sequence: `ESC` `intermediates` `byte`.
    fn esc(&mut self, intermediates: &[u8], byte: u8);
    /// An operating system command, split at its semicolons; `bell` when it ended with BEL rather than ST.
    fn osc(&mut self, params: &[&[u8]], bell: bool);
    /// A device control string with its `data`, once it ended.
    fn dcs(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8, data: &[u8]);
}

/// The parameters of a sequence, each with its subparameters as `38:2::255:0:0` writes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params {
    values: Vec<u16>,
    /// Where each parameter's values start in `values`; its subparameters follow it.
    starts: Vec<u8>,
}

impl Params {
    fn clear(&mut self) {
        self.values.clear();
        self.starts.clear();
    }

    fn push(&mut self, value: u16) {
        if self.starts.len() < MAX_PARAMS {
            self.starts.push(u8::try_from(self.values.len()).unwrap_or(u8::MAX));
            self.values.push(value);
        }
    }

    fn extend(&mut self, value: u16) {
        if self.values.len() < usize::from(u8::MAX) {
            self.values.push(value);
        }
    }

    /// How many parameters there are.
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Parameter `index` with its subparameters.
    pub fn get(&self, index: usize) -> Option<&[u16]> {
        let start = usize::from(*self.starts.get(index)?);
        let end = self
            .starts
            .get(index + 1)
            .map_or(self.values.len(), |&end| usize::from(end));
        self.values.get(start..end)
    }

    /// The first value of parameter `index`, or `default` when it is missing or 0.
    pub fn or(&self, index: usize, default: u16) -> u16 {
        match self.get(index).and_then(|values| values.first()) {
            Some(&0) | None => default,
            Some(&value) => value,
        }
    }

    /// The parameters with their subparameters, in order.
    pub fn iter(&self) -> impl Iterator<Item = &[u16]> {
        (0..self.len()).filter_map(|index| self.get(index))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    DcsEntry,
    DcsParam,
    DcsIntermediate,
    DcsPassthrough,
    DcsIgnore,
    OscString,
    /// SOS, PM and APC strings, which nothing reads.
    SosPmApc,
}

/// The parser's state between two reads.
#[derive(Debug)]
pub struct Parser {
    state: State,
    params: Params,
    /// The value of the parameter being read.
    current: u16,
    /// The parameter being read is a subparameter, after a `:`.
    sub: bool,
    /// Anything of the parameters was seen, so the one being read counts even without digits.
    any_param: bool,
    intermediates: Vec<u8>,
    private: Option<u8>,
    osc: Vec<u8>,
    dcs: Vec<u8>,
    dcs_action: u8,
    /// An ESC inside a string, which ends it if `\` follows.
    string_escape: bool,
    utf8: utf8::Decoder,
}

impl Default for Parser {
    fn default() -> Parser {
        Parser::new()
    }
}

impl Parser {
    /// A parser in the ground state.
    pub fn new() -> Parser {
        Parser {
            state: State::Ground,
            params: Params::default(),
            current: 0,
            sub: false,
            any_param: false,
            intermediates: Vec::new(),
            private: None,
            osc: Vec::new(),
            dcs: Vec::new(),
            dcs_action: 0,
            string_escape: false,
            utf8: utf8::Decoder::default(),
        }
    }

    /// Parses `bytes`, which may end inside a sequence or a character, into `performer`.
    pub fn advance<P: Perform>(&mut self, performer: &mut P, bytes: &[u8]) {
        let mut at = 0;
        while at < bytes.len() {
            if self.state == State::Ground && !self.utf8.pending() {
                at += ground_run(performer, &bytes[at..]);
                if at >= bytes.len() {
                    break;
                }
                // Most escapes open a control sequence, entered here in one step.
                if bytes[at] == 0x1b && bytes.get(at + 1) == Some(&b'[') {
                    self.clear_sequence();
                    self.state = State::CsiEntry;
                    at += 2;
                    if at >= bytes.len() {
                        break;
                    }
                }
            }
            if matches!(self.state, State::CsiEntry | State::CsiParam) {
                let digits = bytes[at..]
                    .iter()
                    .take_while(|&&byte| byte.is_ascii_digit() || byte == b';' || byte == b':')
                    .count();
                if digits > 0 {
                    for &byte in &bytes[at..at + digits] {
                        self.param(byte);
                    }
                    self.state = State::CsiParam;
                    at += digits;
                    continue;
                }
            }
            self.byte(performer, bytes[at]);
            at += 1;
        }
    }

    fn byte<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if self.state == State::Ground {
            return self.ground(performer, byte);
        }
        // CAN and SUB abort any sequence; ESC starts a new one, except inside strings, where it may begin ST.
        match byte {
            0x18 | 0x1a => {
                self.state = State::Ground;
                return performer.execute(byte);
            }
            0x1b if !self.in_string() => return self.enter_escape(),
            _ => {}
        }
        match self.state {
            State::Ground => {}
            State::Escape => self.escape(performer, byte),
            State::EscapeIntermediate => self.escape_intermediate(performer, byte),
            State::CsiEntry | State::CsiParam | State::CsiIntermediate | State::CsiIgnore => self.csi(performer, byte),
            State::DcsEntry | State::DcsParam | State::DcsIntermediate => self.dcs_head(byte),
            State::DcsPassthrough | State::DcsIgnore => self.dcs_data(performer, byte),
            State::OscString => self.osc_byte(performer, byte),
            State::SosPmApc => self.ignored_string(performer, byte),
        }
    }

    fn in_string(&self) -> bool {
        matches!(
            self.state,
            State::OscString | State::DcsPassthrough | State::DcsIgnore | State::SosPmApc
        )
    }

    fn ground<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if self.utf8.pending() && !utf8::Decoder::continues(byte) {
            // The character broke off: its replacement shows, and the byte counts on its own.
            self.utf8 = utf8::Decoder::default();
            performer.print(char::REPLACEMENT_CHARACTER);
        }
        if self.utf8.pending() || byte >= 0x80 {
            if !self.utf8.pending() && (0x80..=0x9f).contains(&byte) {
                // A lone C1 control is executed, as `vte` does, rather than shown as a broken character.
                return performer.execute(byte);
            }
            match self.utf8.push(byte) {
                Some(c @ '\u{80}'..='\u{9f}') => performer.execute(c as u8),
                Some(c) => performer.print(c),
                None => {}
            }
            return;
        }
        match byte {
            0x1b => self.enter_escape(),
            0x00..=0x1f => performer.execute(byte),
            0x7f => {}
            _ => performer.print(char::from(byte)),
        }
    }

    fn enter_escape(&mut self) {
        self.state = State::Escape;
        self.intermediates.clear();
        self.utf8 = utf8::Decoder::default();
    }

    fn clear_sequence(&mut self) {
        self.params.clear();
        self.current = 0;
        self.sub = false;
        self.any_param = false;
        self.intermediates.clear();
        self.private = None;
    }

    fn escape<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        match byte {
            b'[' => {
                self.clear_sequence();
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc.clear();
                self.string_escape = false;
                self.state = State::OscString;
            }
            b'P' => {
                self.clear_sequence();
                self.dcs.clear();
                self.string_escape = false;
                self.state = State::DcsEntry;
            }
            b'X' | b'^' | b'_' => {
                self.string_escape = false;
                self.state = State::SosPmApc;
            }
            0x20..=0x2f => {
                self.intermediates.push(byte);
                self.state = State::EscapeIntermediate;
            }
            0x00..=0x1f => performer.execute(byte),
            0x30..=0x7e => {
                self.state = State::Ground;
                performer.esc(&[], byte);
            }
            _ => self.state = State::Ground,
        }
    }

    fn escape_intermediate<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        match byte {
            0x20..=0x2f => {
                if self.intermediates.len() < 2 {
                    self.intermediates.push(byte);
                }
            }
            0x00..=0x1f => performer.execute(byte),
            0x30..=0x7e => {
                self.state = State::Ground;
                performer.esc(&self.intermediates, byte);
            }
            _ => self.state = State::Ground,
        }
    }

    /// Takes a digit or separator of the parameters; whether `byte` was one.
    fn param(&mut self, byte: u8) -> bool {
        match byte {
            b'0'..=b'9' => {
                self.current = self.current.saturating_mul(10).saturating_add(u16::from(byte - b'0'));
                self.any_param = true;
            }
            b';' => {
                self.commit_param();
                self.sub = false;
                self.any_param = true;
            }
            b':' => {
                self.commit_param();
                self.sub = true;
                self.any_param = true;
            }
            _ => return false,
        }
        true
    }

    fn commit_param(&mut self) {
        if self.sub {
            self.params.extend(self.current);
        } else {
            self.params.push(self.current);
        }
        self.current = 0;
    }

    /// Ends the parameters before the action, the last one counting when anything of them was there.
    fn finish_params(&mut self) {
        if self.any_param {
            self.commit_param();
            self.any_param = false;
        }
    }

    fn csi<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if byte < 0x20 {
            return performer.execute(byte);
        }
        if self.state == State::CsiIgnore {
            if (0x40..=0x7e).contains(&byte) {
                self.state = State::Ground;
            }
            return;
        }
        match byte {
            0x3c..=0x3f if self.state == State::CsiEntry => {
                self.private = Some(byte);
                self.state = State::CsiParam;
            }
            b'0'..=b'9' | b';' | b':' if self.state != State::CsiIntermediate => {
                self.param(byte);
                self.state = State::CsiParam;
            }
            0x20..=0x2f => {
                if self.intermediates.len() >= 2 {
                    self.state = State::CsiIgnore;
                    return;
                }
                self.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7e => {
                self.finish_params();
                self.state = State::Ground;
                performer.csi(&self.params, &self.intermediates, self.private, byte);
            }
            0x7f => {}
            _ => self.state = State::CsiIgnore,
        }
    }

    fn dcs_head(&mut self, byte: u8) {
        match byte {
            0x00..=0x1f | 0x7f => {}
            0x3c..=0x3f if self.state == State::DcsEntry => {
                self.private = Some(byte);
                self.state = State::DcsParam;
            }
            b'0'..=b'9' | b';' | b':' if self.state != State::DcsIntermediate => {
                self.param(byte);
                self.state = State::DcsParam;
            }
            0x20..=0x2f => {
                if self.intermediates.len() >= 2 {
                    self.state = State::DcsIgnore;
                    return;
                }
                self.intermediates.push(byte);
                self.state = State::DcsIntermediate;
            }
            0x40..=0x7e => {
                self.finish_params();
                self.dcs_action = byte;
                self.state = State::DcsPassthrough;
            }
            _ => self.state = State::DcsIgnore,
        }
    }

    fn dcs_data<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if self.string_escape {
            return self.after_string_escape(performer, byte);
        }
        match byte {
            // Like an OSC, a DCS ends at any ESC; ESC \ is its proper end.
            0x1b => {
                self.dispatch_dcs(performer);
                self.string_escape = true;
            }
            0x9c => {
                self.dispatch_dcs(performer);
                self.state = State::Ground;
            }
            _ if self.state == State::DcsPassthrough => {
                if self.dcs.len() < MAX_DCS {
                    self.dcs.push(byte);
                } else {
                    self.state = State::DcsIgnore;
                }
            }
            _ => {}
        }
    }

    fn dispatch_dcs<P: Perform>(&mut self, performer: &mut P) {
        if self.state == State::DcsPassthrough {
            performer.dcs(
                &self.params,
                &self.intermediates,
                self.private,
                self.dcs_action,
                &self.dcs,
            );
        }
    }

    /// The byte after an ESC that ended a string: the `\` of ST is taken, anything else starts an escape sequence.
    fn after_string_escape<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        self.string_escape = false;
        if byte == b'\\' {
            self.state = State::Ground;
        } else {
            self.enter_escape();
            self.byte(performer, byte);
        }
    }

    fn osc_byte<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if self.string_escape {
            return self.after_string_escape(performer, byte);
        }
        match byte {
            0x07 => {
                self.dispatch_osc(performer, true);
                self.state = State::Ground;
            }
            // Like xterm, an ESC ends the string whatever follows; ESC \ is its proper end.
            0x1b => {
                self.dispatch_osc(performer, false);
                self.string_escape = true;
            }
            0x00..=0x06 | 0x08..=0x1a | 0x1c..=0x1f => {}
            _ => {
                if self.osc.len() < MAX_OSC {
                    self.osc.push(byte);
                }
            }
        }
    }

    fn dispatch_osc<P: Perform>(&mut self, performer: &mut P, bell: bool) {
        let mut parts: Vec<&[u8]> = Vec::with_capacity(4);
        let osc = &self.osc;
        // Only the first part is split by number; a title or URL may hold semicolons of its own beyond the
        // parts its command knows, which the terminal joins again where it needs to.
        parts.extend(osc.split(|&b| b == b';'));
        performer.osc(&parts, bell);
    }

    fn ignored_string<P: Perform>(&mut self, performer: &mut P, byte: u8) {
        if self.string_escape {
            return self.after_string_escape(performer, byte);
        }
        match byte {
            0x1b => self.string_escape = true,
            0x9c => self.state = State::Ground,
            _ => {}
        }
    }
}

/// In the ground state: hands the printable ASCII at the start of `bytes` over at once; how many bytes it took.
fn ground_run<P: Perform>(performer: &mut P, bytes: &[u8]) -> usize {
    let ascii = bytes
        .iter()
        .position(|&byte| !(0x20..0x7f).contains(&byte))
        .unwrap_or(bytes.len());
    if ascii == bytes.len() || bytes[ascii] < 0x80 {
        if ascii > 0 {
            performer.print_ascii(&bytes[..ascii]);
        }
        return ascii;
    }
    // Whole characters decode in one pass; a broken or cut-off one, a control and C1 are left to the byte-wise
    // path.
    let mut at = ascii;
    let mut ascii_from = 0;
    let mut run = ['\0'; RUN];
    let mut held = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte < 0x80 {
            if byte < 0x20 || byte == 0x7f {
                break;
            }
            at += 1;
            continue;
        }
        let Some((c, length)) = decode_char(&bytes[at..]) else {
            break;
        };
        if ascii_from < at || held == RUN || ('\u{80}'..='\u{9f}').contains(&c) {
            if held > 0 {
                performer.print_chars(&run[..held]);
                held = 0;
            }
            if ascii_from < at {
                performer.print_ascii(&bytes[ascii_from..at]);
            }
            if ('\u{80}'..='\u{9f}').contains(&c) {
                return at;
            }
        }
        run[held] = c;
        held += 1;
        at += length;
        ascii_from = at;
    }
    if held > 0 {
        performer.print_chars(&run[..held]);
    }
    if ascii_from < at {
        performer.print_ascii(&bytes[ascii_from..at]);
    }
    at
}

/// The most characters beyond ASCII the parser hands over at once.
const RUN: usize = 256;

/// The character `bytes` start with and its length, where they start with a whole, shortest, valid one.
#[inline]
fn decode_char(bytes: &[u8]) -> Option<(char, usize)> {
    // Most text beyond ASCII is three bytes long (CJK and most scripts); those without a special case take the
    // short way.
    if let [first @ (0xe1..=0xec | 0xee..=0xef), second, third, ..] = *bytes
        && second & 0xc0 == 0x80
        && third & 0xc0 == 0x80
    {
        let value = (u32::from(first & 0x0f) << 12) | (u32::from(second & 0x3f) << 6) | u32::from(third & 0x3f);
        return char::from_u32(value).map(|c| (c, 3));
    }
    let continuation = |at: usize| {
        bytes
            .get(at)
            .filter(|&&byte| byte & 0xc0 == 0x80)
            .map(|&byte| u32::from(byte & 0x3f))
    };
    let first = u32::from(*bytes.first()?);
    let (value, length) = match first {
        0xc2..=0xdf => (((first & 0x1f) << 6) | continuation(1)?, 2),
        0xe0..=0xef => {
            let second = continuation(1)?;
            if (first == 0xe0 && second < 0x20) || (first == 0xed && second >= 0x20) {
                return None;
            }
            (((first & 0x0f) << 12) | (second << 6) | continuation(2)?, 3)
        }
        0xf0..=0xf4 => {
            let second = continuation(1)?;
            if (first == 0xf0 && second < 0x10) || (first == 0xf4 && second >= 0x10) {
                return None;
            }
            let value = ((first & 0x07) << 18) | (second << 12) | (continuation(2)? << 6) | continuation(3)?;
            (value, 4)
        }
        _ => return None,
    };
    Some((char::from_u32(value)?, length))
}

#[cfg(test)]
mod utf8_tests {
    #[test]
    fn the_decoder_takes_exactly_the_characters_the_standard_library_takes() {
        let tails = [0x00u8, 0x41, 0x7f, 0x80, 0x8f, 0x90, 0x9f, 0xa0, 0xbf, 0xc0, 0xff];
        for first in 0x80..=0xffu8 {
            for &second in &tails {
                for &third in &tails {
                    for &fourth in &tails {
                        let bytes = [first, second, third, fourth];
                        let expected = (2..=4).find_map(|length| {
                            std::str::from_utf8(&bytes[..length])
                                .ok()
                                .and_then(|text| text.chars().next())
                                .map(|c| (c, length))
                        });
                        assert_eq!(super::decode_char(&bytes), expected, "{bytes:02x?}");
                    }
                }
            }
        }
    }
}
