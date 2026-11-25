//! The parser tests of `vte` 0.15 (`src/lib.rs`), ported to this parser.
//!
//! Skipped:
//! - `build_with_fixed_size`: `vte`'s const-generic OSC buffer size; this parser has one size, and the input is
//!   `csi_reset`'s.
//! - `exceed_fixed_osc_buffer_size`: the same const-generic buffer; the cut at this parser's limit is
//!   `osc_longer_than_its_buffer_is_cut_at_the_limit`.
//!
//! `vte`'s `ignore` flag and its `hook`/`put`/`unhook` streaming have no counterpart: a sequence past the limits is
//! still dispatched with what was kept, and a DCS arrives whole.

use super::*;

const OSC_BYTES: &[u8] = b"\x1b]2;jwilm@jwilm-desk: ~/code/alacritty\x07";

#[derive(Debug, PartialEq, Eq)]
enum Sequence {
    Osc(Vec<Vec<u8>>, bool),
    Csi(Vec<Vec<u16>>, Vec<u8>, Option<u8>, u8),
    Esc(Vec<u8>, u8),
    Dcs(Vec<Vec<u16>>, Vec<u8>, Option<u8>, u8, Vec<u8>),
    Print(char),
    Execute(u8),
}

#[derive(Default)]
struct Dispatcher(Vec<Sequence>);

fn params_of(params: &Params) -> Vec<Vec<u16>> {
    params.iter().map(<[u16]>::to_vec).collect()
}

impl Perform for Dispatcher {
    fn print(&mut self, c: char) {
        self.0.push(Sequence::Print(c));
    }

    fn execute(&mut self, byte: u8) {
        self.0.push(Sequence::Execute(byte));
    }

    fn csi(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8) {
        self.0.push(Sequence::Csi(
            params_of(params),
            intermediates.to_vec(),
            private,
            action,
        ));
    }

    fn esc(&mut self, intermediates: &[u8], byte: u8) {
        self.0.push(Sequence::Esc(intermediates.to_vec(), byte));
    }

    fn osc(&mut self, params: &[&[u8]], bell: bool) {
        self.0
            .push(Sequence::Osc(params.iter().map(|p| p.to_vec()).collect(), bell));
    }

    fn dcs(&mut self, params: &Params, intermediates: &[u8], private: Option<u8>, action: u8, data: &[u8]) {
        self.0.push(Sequence::Dcs(
            params_of(params),
            intermediates.to_vec(),
            private,
            action,
            data.to_vec(),
        ));
    }
}

fn parse(chunks: &[&[u8]]) -> Vec<Sequence> {
    let mut parser = Parser::new();
    let mut dispatcher = Dispatcher::default();
    for chunk in chunks {
        parser.advance(&mut dispatcher, chunk);
    }
    dispatcher.0
}

fn prints(text: &str) -> Vec<Sequence> {
    text.chars().map(Sequence::Print).collect()
}

// vte: parse_osc
#[test]
fn osc_splits_into_command_and_text() {
    assert_eq!(
        parse(&[OSC_BYTES]),
        [Sequence::Osc(
            vec![b"2".to_vec(), OSC_BYTES[4..OSC_BYTES.len() - 1].to_vec()],
            true
        )]
    );
}

// vte: parse_empty_osc
#[test]
fn empty_osc_is_dispatched() {
    assert_eq!(parse(&[b"\x1b]\x07"]), [Sequence::Osc(vec![Vec::new()], true)]);
}

// vte: parse_osc_max_params
#[test]
fn osc_keeps_every_part_and_ends_at_esc() {
    let input = format!("\x1b]{}\x1b", ";".repeat(MAX_PARAMS + 1));
    assert_eq!(
        parse(&[input.as_bytes()]),
        [Sequence::Osc(vec![Vec::new(); MAX_PARAMS + 2], false)]
    );
}

// vte: osc_bell_terminated
#[test]
fn osc_ended_by_bell_says_so() {
    assert!(matches!(
        parse(&[b"\x1b]11;ff/00/ff\x07"]).as_slice(),
        [Sequence::Osc(_, true)]
    ));
}

// vte: osc_c0_st_terminated
#[test]
fn osc_ended_by_st_takes_the_whole_terminator() {
    assert!(matches!(
        parse(&[b"\x1b]11;ff/00/ff\x1b\\"]).as_slice(),
        [Sequence::Osc(_, false)]
    ));
}

// vte: parse_osc_with_utf8_arguments
#[test]
fn osc_carries_utf8_text() {
    let input = "\r\x1b]2;echo '¯\\_(ツ)_/¯' && sleep 1\x07".as_bytes();
    assert_eq!(
        parse(&[input]),
        [
            Sequence::Execute(b'\r'),
            Sequence::Osc(vec![b"2".to_vec(), input[5..input.len() - 1].to_vec()], true),
        ]
    );
}

// vte: osc_containing_string_terminator
#[test]
fn osc_utf8_with_an_st_byte_inside_is_not_cut() {
    let input = b"\x1b]2;\xe6\x9c\xab\x1b\\";
    assert_eq!(
        parse(&[input]),
        [Sequence::Osc(vec![b"2".to_vec(), "末".as_bytes().to_vec()], false)]
    );
}

// vte: fixed_size_osc_containing_string_terminator
#[test]
fn osc_utf8_with_an_st_byte_inside_across_reads() {
    assert_eq!(
        parse(&[b"\x1b]2;", b"s\xe6\x9c\xab", b"\x1b\\"]),
        [Sequence::Osc(vec![b"2".to_vec(), "s末".as_bytes().to_vec()], false)]
    );
}

// vte: exceed_max_buffer_size
#[test]
fn osc_longer_than_its_buffer_is_cut_at_the_limit() {
    let mut parser = Parser::new();
    let mut dispatcher = Dispatcher::default();
    parser.advance(&mut dispatcher, b"\x1b]52;s");
    parser.advance(&mut dispatcher, &vec![b'a'; MAX_OSC + 100]);
    parser.advance(&mut dispatcher, b"\x07");
    let [Sequence::Osc(params, true)] = dispatcher.0.as_slice() else {
        panic!("expected one OSC, got {:?}", dispatcher.0.len());
    };
    assert_eq!(params.len(), 2);
    assert_eq!(params[0], b"52");
    assert_eq!(params[1].len(), MAX_OSC - b"52;".len());
}

// vte: parse_csi_max_params
#[test]
fn csi_keeps_the_most_parameters() {
    let input = format!("\x1b[{}p", "1;".repeat(MAX_PARAMS - 1));
    let mut expected = vec![vec![1]; MAX_PARAMS - 1];
    expected.push(vec![0]);
    assert_eq!(
        parse(&[input.as_bytes()]),
        [Sequence::Csi(expected, Vec::new(), None, b'p')]
    );
}

// vte: parse_csi_params_ignore_long_params
#[test]
fn csi_drops_parameters_past_the_most() {
    let input = format!("\x1b[{}p", "1;".repeat(MAX_PARAMS));
    assert_eq!(
        parse(&[input.as_bytes()]),
        [Sequence::Csi(vec![vec![1]; MAX_PARAMS], Vec::new(), None, b'p')]
    );
}

// vte: parse_csi_params_trailing_semicolon
#[test]
fn csi_trailing_semicolon_adds_an_empty_parameter() {
    assert_eq!(
        parse(&[b"\x1b[4;m"]),
        [Sequence::Csi(vec![vec![4], vec![0]], Vec::new(), None, b'm')]
    );
}

// vte: parse_csi_params_leading_semicolon
#[test]
fn csi_leading_semicolon_adds_an_empty_parameter() {
    assert_eq!(
        parse(&[b"\x1b[;4m"]),
        [Sequence::Csi(vec![vec![0], vec![4]], Vec::new(), None, b'm')]
    );
}

// vte: parse_long_csi_param
#[test]
fn csi_parameter_too_large_saturates() {
    assert_eq!(
        parse(&[b"\x1b[9223372036854775808m"]),
        [Sequence::Csi(vec![vec![u16::MAX]], Vec::new(), None, b'm')]
    );
}

// vte: csi_reset
#[test]
fn csi_interrupted_by_another_starts_afresh() {
    assert_eq!(
        parse(&[b"\x1b[3;1\x1b[?1049h"]),
        [Sequence::Csi(vec![vec![1049]], Vec::new(), Some(b'?'), b'h')]
    );
}

// vte: csi_subparameters
#[test]
fn csi_subparameters_stay_with_their_parameter() {
    assert_eq!(
        parse(&[b"\x1b[38:2:255:0:255;1m"]),
        [Sequence::Csi(
            vec![vec![38, 2, 255, 0, 255], vec![1]],
            Vec::new(),
            None,
            b'm'
        )]
    );
}

// vte: params_buffer_filled_with_subparam
#[test]
fn csi_subparameters_do_not_count_against_the_most_parameters() {
    let input = format!("\x1b[{}x\x1b", ":".repeat(MAX_PARAMS));
    assert_eq!(
        parse(&[input.as_bytes()]),
        [Sequence::Csi(vec![vec![0; MAX_PARAMS + 1]], Vec::new(), None, b'x')]
    );
}

// vte: parse_dcs_max_params
#[test]
fn dcs_drops_parameters_past_the_most() {
    let input = format!("\x1bP{}p\x1b\\", "1;".repeat(MAX_PARAMS + 1));
    assert_eq!(
        parse(&[input.as_bytes()]),
        [Sequence::Dcs(
            vec![vec![1]; MAX_PARAMS],
            Vec::new(),
            None,
            b'p',
            Vec::new()
        )]
    );
}

// vte: dcs_reset
#[test]
fn dcs_after_an_interrupted_csi_starts_afresh() {
    assert_eq!(
        parse(&[b"\x1b[3;1\x1bP1$tx\x9c"]),
        [Sequence::Dcs(vec![vec![1]], b"$".to_vec(), None, b't', b"x".to_vec())]
    );
}

// vte: parse_dcs
#[test]
fn dcs_carries_parameters_action_and_data() {
    assert_eq!(
        parse(&[b"\x1bP0;1|17/ab\x9c"]),
        [Sequence::Dcs(
            vec![vec![0], vec![1]],
            Vec::new(),
            None,
            b'|',
            b"17/ab".to_vec()
        )]
    );
}

// vte: intermediate_reset_on_dcs_exit
#[test]
fn dcs_ended_by_another_escape_leaves_no_intermediates_behind() {
    assert_eq!(
        parse(&[b"\x1bP=1sZZZ\x1b+\\"]),
        [
            Sequence::Dcs(vec![vec![1]], Vec::new(), Some(b'='), b's', b"ZZZ".to_vec()),
            Sequence::Esc(b"+".to_vec(), b'\\'),
        ]
    );
    assert_eq!(
        parse(&[b"\x1bP1$sZZZ\x1b+\\"]),
        [
            Sequence::Dcs(vec![vec![1]], b"$".to_vec(), None, b's', b"ZZZ".to_vec()),
            Sequence::Esc(b"+".to_vec(), b'\\'),
        ]
    );
}

// vte: esc_reset
#[test]
fn esc_after_an_interrupted_csi_starts_afresh() {
    assert_eq!(parse(&[b"\x1b[3;1\x1b(A"]), [Sequence::Esc(b"(".to_vec(), b'A')]);
}

// vte: esc_reset_intermediates
#[test]
fn esc_after_a_private_csi_has_only_its_own_intermediates() {
    assert_eq!(
        parse(&[b"\x1b[?2004l\x1b#8"]),
        [
            Sequence::Csi(vec![vec![2004]], Vec::new(), Some(b'?'), b'l'),
            Sequence::Esc(b"#".to_vec(), b'8'),
        ]
    );
}

// vte: unicode
#[test]
fn four_byte_characters_print() {
    assert_eq!(parse(&["🎉_🦀🦀_🎉".as_bytes()]), prints("🎉_🦀🦀_🎉"));
}

// vte: invalid_utf8
#[test]
fn broken_character_prints_a_replacement() {
    assert_eq!(parse(&[b"a\xef\xbcb"]), prints("a\u{fffd}b"));
}

// vte: partial_utf8
#[test]
fn character_split_over_reads_prints_once() {
    let input = "🚀".as_bytes();
    assert_eq!(
        parse(&[&input[..1], &input[1..2], &input[2..3], &input[3..]]),
        prints("🚀")
    );
}

// vte: partial_utf8_separating_utf8
#[test]
fn split_character_followed_by_another_in_the_same_read() {
    let input = "ĸ🎉".as_bytes();
    assert_eq!(parse(&[&input[..1], &input[1..]]), prints("ĸ🎉"));
}

// vte: partial_invalid_utf8
#[test]
fn broken_character_split_over_reads_prints_a_replacement() {
    let input = b"a\xef\xbcb";
    assert_eq!(
        parse(&[&input[..1], &input[1..2], &input[2..3], &input[3..]]),
        prints("a\u{fffd}b")
    );
}

// vte: partial_invalid_utf8_split
#[test]
fn stray_continuation_byte_after_a_split_character() {
    let input = b"\xe4\xbf\x99\xb5";
    assert_eq!(parse(&[&input[..2], &input[2..]]), prints("俙\u{fffd}"));
}

// vte: partial_utf8_into_esc
#[test]
fn character_broken_off_by_esc_prints_a_replacement_and_the_escape_runs() {
    assert_eq!(
        parse(&[b"\xd8\x1b012"]),
        [
            Sequence::Print('\u{fffd}'),
            Sequence::Esc(Vec::new(), b'0'),
            Sequence::Print('1'),
            Sequence::Print('2'),
        ]
    );
}

// vte: c1s
#[test]
fn c0_and_lone_c1_controls_are_executed() {
    let input = b"\x00\x1f\x80\x90\x98\x9b\x9c\x9d\x9e\x9fa";
    let mut expected: Vec<Sequence> = input[..10].iter().map(|&byte| Sequence::Execute(byte)).collect();
    expected.push(Sequence::Print('a'));
    assert_eq!(parse(&[input]), expected);
}

// vte: execute_anywhere
#[test]
fn cancel_and_substitute_are_executed() {
    assert_eq!(
        parse(&[b"\x18\x1a"]),
        [Sequence::Execute(0x18), Sequence::Execute(0x1a)]
    );
}
