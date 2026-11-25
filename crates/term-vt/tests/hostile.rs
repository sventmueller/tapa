//! Output written to harm the user (F182) and output no program writes (F183): what the core answers, and that no
//! bytes make it panic.

use term_vt::{Event, Size, Terminal};

fn terminal() -> Terminal {
    Terminal::new(Size { columns: 40, lines: 10 }, 100)
}

fn replies(terminal: &mut Terminal) -> Vec<Vec<u8>> {
    terminal
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Reply(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

#[test]
fn the_title_is_never_reported_back_where_it_would_be_typed_as_input() {
    let mut term = terminal();
    term.advance(b"\x1b]2;rm -rf ~\x07\x1b]1;icon\x07");
    for query in [&b"\x1b[21t"[..], b"\x1b[20t", b"\x1b[19t", b"\x1b]l\x1b\\"] {
        term.advance(query);
        assert_eq!(replies(&mut term), Vec::<Vec<u8>>::new(), "{query:?}");
    }
}

#[test]
fn requests_that_echo_text_the_program_chose_get_no_answer() {
    let mut term = terminal();
    for query in [
        &b"\x1bP$qm\x1b\\"[..],
        b"\x1bP$q\"p\x1b\\",
        b"\x1bP+q544e\x1b\\",
        b"\x1b]50;?\x07",
        b"\x1b]52;c;?\x07",
    ] {
        term.advance(query);
        assert!(term.take_events().is_empty(), "{query:?}");
    }
}

#[test]
fn reading_the_clipboard_needs_the_owners_consent_and_writing_it_takes_only_text() {
    let mut term = terminal();
    term.advance(b"\x1b]52;c;?\x07");
    assert!(term.take_events().is_empty());
    term.allow_clipboard_read(true);
    term.advance(b"\x1b]52;c;?\x07");
    assert_eq!(term.take_events(), [Event::ClipboardLoad(b'c', "\x07")]);
    term.advance(b"\x1b]52;c;/w==\x07");
    assert!(term.take_events().is_empty(), "bytes that are not UTF-8");
}

#[test]
fn a_title_cannot_carry_control_characters_into_the_window() {
    let mut term = terminal();
    term.advance(b"\x1b]2;one\x1b[31mtwo\x07");
    let titles: Vec<Option<String>> = term
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Title(title) => Some(title),
            _ => None,
        })
        .collect();
    assert!(
        titles
            .iter()
            .flatten()
            .all(|title| !title.chars().any(char::is_control)),
        "{titles:?}"
    );
}

/// A small generator, so the cases are the same on every run and a failure names its seed.
struct Bytes(u64);

impl Bytes {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> usize {
        usize::try_from(self.next() % n).unwrap_or(0)
    }

    /// Output with many introducers, parameters and terminators, where the parser's states change.
    fn output(&mut self) -> Vec<u8> {
        const PIECES: &[&[u8]] = &[
            b"\x1b",
            b"\x1b[",
            b"\x1b]",
            b"\x1bP",
            b"\x1b_",
            b"\x1b^",
            b"\x1bX",
            b"\x07",
            b"\x1b\\",
            b"\x9b",
            b"\x9d",
            b"\x90",
            b"\x9c",
            b";",
            b":",
            b"?",
            b">",
            b"$",
            b"!",
            b"\"",
            b"*",
            b"0",
            b"1",
            b"9",
            b"65535",
            b"4294967296",
            b"m",
            b"t",
            b"q",
            b"p",
            b"y",
            b"H",
            b"J",
            b"r",
            b"h",
            b"l",
            b"52;c;",
            b"8;;",
            b"133;",
            b"7;file://",
            b"\n",
            b"\r",
            b"\t",
            b"\x08",
            "漢".as_bytes(),
            "e\u{301}".as_bytes(),
            b"\xf0\x9f",
            b"\xff",
            b"a",
        ];
        let mut out = Vec::new();
        for _ in 0..self.below(200) {
            if self.below(4) == 0 {
                out.push(u8::try_from(self.next() & 0xff).unwrap_or(0));
            } else {
                out.extend_from_slice(PIECES[self.below(PIECES.len() as u64)]);
            }
        }
        out
    }
}

#[test]
fn no_output_makes_the_core_panic() {
    let cases: u64 = std::env::var("TERM_FUZZ_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(3000);
    let seed: u64 = std::env::var("TERM_FUZZ_SEED")
        .ok()
        .and_then(|seed| seed.parse().ok())
        .unwrap_or(0x5eed);
    for case in 0..cases {
        let seed = seed.wrapping_add(case).max(1);
        let mut bytes = Bytes(seed);
        let mut term = Terminal::new(
            Size {
                columns: 1 + bytes.below(30),
                lines: 1 + bytes.below(12),
            },
            50,
        );
        term.allow_clipboard_read(true);
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for _ in 0..4 {
                let output = bytes.output();
                let cut = bytes.below(output.len() as u64 + 1);
                term.advance(&output[..cut]);
                term.advance(&output[cut..]);
                if bytes.below(3) == 0 {
                    term.resize(Size {
                        columns: 1 + bytes.below(30),
                        lines: 1 + bytes.below(12),
                    });
                }
                let _ = term.take_events();
            }
        }));
        assert!(run.is_ok(), "TERM_FUZZ_SEED={seed} TERM_FUZZ_CASES=1");
    }
}
