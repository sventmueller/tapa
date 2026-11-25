//! The ANSI handler tests of `vte` 0.15 (`src/ansi.rs`), ported as behaviour tests of the terminal: what a sequence
//! leaves on the screen, in the colours and modes, and in the events.
//!
//! Skipped:
//! - `sync_bursts_buffer`: the core buffers nothing during a synchronized update; it only sets the mode, and the
//!   session holds frames back with a timeout.
//! - `contrast`: `vte`'s `Rgb::contrast`, which `term-vt` has no counterpart of; contrast is the theme's business.
//!
//! The other synchronized-update tests check `vte`'s buffering; here they check the mode and that what came inside
//! the update took effect.

use term_vt::Terminal;
use term_vt::color::Color;
use term_vt::style::{Attrs, Style};
use term_vt::term::{Event, Modes, Size};

fn terminal() -> Terminal {
    Terminal::new(Size { columns: 80, lines: 4 }, 0)
}

fn style_at(terminal: &Terminal, line: usize, column: usize) -> Style {
    terminal.style(terminal.grid().rows[line].cells[column].style)
}

fn text_of(terminal: &Terminal, line: usize) -> String {
    let text: String = terminal.grid().rows[line].cells.iter().map(|cell| cell.c).collect();
    text.trim_end().to_string()
}

fn identified(bytes: &[u8]) -> bool {
    let mut terminal = terminal();
    terminal.advance(bytes);
    terminal.take_events().contains(&Event::Reply(b"\x1b[?6c".to_vec()))
}

/// The colour `spec` sets entry 1 of the table to, through OSC 4.
fn osc4(spec: &str) -> Option<(u8, u8, u8)> {
    let mut terminal = terminal();
    terminal.advance(format!("\x1b]4;1;{spec}\x1b\\").as_bytes());
    terminal.color_changes()[1]
}

fn nothing_changed(terminal: &Terminal) -> bool {
    terminal.color_changes().iter().all(Option::is_none)
}

// vte: parse_control_attribute
#[test]
fn sgr_1_makes_text_bold() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b[1mx");
    assert!(style_at(&terminal, 0, 0).attrs.contains(Attrs::BOLD));
}

// vte: parse_terminal_identity_csi
#[test]
fn primary_device_attributes_answer_only_without_a_parameter_or_with_0() {
    assert!(!identified(b"\x1b[1c"));
    assert!(identified(b"\x1b[c"));
    assert!(identified(b"\x1b[0c"));
}

// vte: parse_terminal_identity_esc
#[test]
fn decid_answers_but_not_with_an_intermediate() {
    assert!(identified(b"\x1bZ"));
    assert!(!identified(b"\x1b#Z"));
}

// vte: parse_truecolor_attr
#[test]
fn sgr_38_2_sets_a_true_colour_foreground() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b[38;2;128;66;255mx");
    assert_eq!(style_at(&terminal, 0, 0).fg, Color::Rgb(128, 66, 255));
}

// vte: parse_zsh_startup
#[test]
fn zsh_startup_leaves_its_prompt() {
    let mut bytes = b"\x1b[1m\x1b[7m%\x1b[27m\x1b[1m\x1b[0m".to_vec();
    bytes.extend(std::iter::repeat_n(b' ', 79));
    bytes.extend_from_slice(b"\r \r\r\x1b[0m\x1b[27m\x1b[24m\x1b[Jjwilm@jwilm-desk \x1b[01;32m");
    bytes.extend_from_slice("➜ \x1b[01;32m \x1b[36m~/code".as_bytes());
    let mut terminal = terminal();
    terminal.advance(&bytes);
    assert_eq!(text_of(&terminal, 0), "jwilm@jwilm-desk ➜  ~/code");
    assert_eq!(text_of(&terminal, 1), "");
    let arrow = style_at(&terminal, 0, 17);
    assert_eq!(arrow.fg, Color::Named(2));
    assert!(arrow.attrs.contains(Attrs::BOLD));
    let path = style_at(&terminal, 0, 20);
    assert_eq!(path.fg, Color::Named(6));
    assert!(path.attrs.contains(Attrs::BOLD));
    assert_eq!((terminal.cursor().line, terminal.cursor().column), (0, 26));
}

// vte: parse_designate_g0_as_line_drawing
#[test]
fn g0_designated_as_line_drawing_draws_lines() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b(0q");
    assert_eq!(text_of(&terminal, 0), "─");
}

// vte: parse_designate_g1_as_line_drawing_and_invoke
#[test]
fn g1_designated_as_line_drawing_draws_lines_once_shifted_out() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b)0q");
    assert_eq!(text_of(&terminal, 0), "q");
    terminal.advance(b"\x0eq");
    assert_eq!(text_of(&terminal, 0), "q─");
}

// vte: parse_valid_rgb_colors
#[test]
fn rgb_colour_specs_scale_each_channel() {
    assert_eq!(osc4("rgb:f/e/d"), Some((0xff, 0xee, 0xdd)));
    assert_eq!(osc4("rgb:11/aa/ff"), Some((0x11, 0xaa, 0xff)));
    assert_eq!(osc4("rgb:f/ed1/cb23"), Some((0xff, 0xec, 0xca)));
    assert_eq!(osc4("rgb:ffff/0/0"), Some((0xff, 0x00, 0x00)));
}

// vte: parse_valid_legacy_rgb_colors
#[test]
fn hash_colour_specs_keep_two_digits_per_channel() {
    assert_eq!(osc4("#1af"), Some((0x10, 0xa0, 0xf0)));
    assert_eq!(osc4("#11aaff"), Some((0x11, 0xaa, 0xff)));
    assert_eq!(osc4("#110aa0ff0"), Some((0x11, 0xaa, 0xff)));
    assert_eq!(osc4("#1100aa00ff00"), Some((0x11, 0xaa, 0xff)));
}

// vte: parse_invalid_rgb_colors
#[test]
fn rgb_colour_specs_with_empty_channels_change_nothing() {
    assert_eq!(osc4("rgb:0//"), None);
    assert_eq!(osc4("rgb://///"), None);
}

// vte: parse_invalid_legacy_rgb_colors
#[test]
fn hash_colour_specs_too_short_change_nothing() {
    assert_eq!(osc4("#"), None);
    assert_eq!(osc4("#f"), None);
}

// vte: parse_invalid_number
#[test]
fn osc4_with_a_malformed_index_changes_nothing() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b]4;1abc;#fff\x1b\\");
    assert!(nothing_changed(&terminal));
}

// vte: parse_valid_number
#[test]
fn osc4_sets_the_entry_it_names() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b]4;123;#fff\x1b\\");
    assert_eq!(terminal.color_changes()[123], Some((0xf0, 0xf0, 0xf0)));
}

// vte: parse_number_too_large
#[test]
fn osc4_with_an_index_past_the_table_changes_nothing() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b]4;321;#fff\x1b\\\x1b]4;256;#fff\x1b\\");
    assert!(nothing_changed(&terminal));
}

// vte: parse_osc4_set_color
#[test]
fn osc4_sets_a_colour_from_a_short_hash_spec() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b]4;0;#fff\x1b\\");
    assert_eq!(terminal.color_changes()[0], Some((0xf0, 0xf0, 0xf0)));
}

/// A terminal with table entries 1 and 2 changed.
fn two_colours_changed() -> Terminal {
    let mut terminal = terminal();
    terminal.advance(b"\x1b]4;1;#ff0000;2;#00ff00\x1b\\");
    assert!(terminal.color_changes()[1].is_some() && terminal.color_changes()[2].is_some());
    terminal
}

// vte: parse_osc104_reset_color
#[test]
fn osc104_with_an_index_resets_only_that_entry() {
    let mut terminal = two_colours_changed();
    terminal.advance(b"\x1b]104;1;\x1b\\");
    assert_eq!(terminal.color_changes()[1], None);
    assert!(terminal.color_changes()[2].is_some());
}

// vte: parse_osc104_reset_all_colors
#[test]
fn osc104_with_an_empty_index_resets_the_table() {
    let mut terminal = two_colours_changed();
    terminal.advance(b"\x1b]104;\x1b\\");
    assert!(nothing_changed(&terminal));
}

// vte: parse_osc104_reset_all_colors_no_semicolon
#[test]
fn osc104_alone_resets_the_table() {
    let mut terminal = two_colours_changed();
    terminal.advance(b"\x1b]104\x1b\\");
    assert!(nothing_changed(&terminal));
}

// vte: partial_sync_updates
#[test]
fn synchronized_update_split_over_reads_starts_extends_and_ends() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b[?20");
    assert!(!terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"26h");
    assert!(terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"random \x1b[31m stuff");
    terminal.advance(b"\x1b[?20");
    terminal.advance(b"26h");
    assert!(terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"\x1b[?20");
    assert!(terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"26l");
    assert!(!terminal.modes().contains(Modes::SYNC));
    assert_eq!(text_of(&terminal, 0), "random  stuff");
    assert_eq!(style_at(&terminal, 0, 8).fg, Color::Named(1));
}

// vte: mixed_sync_escape
#[test]
fn sgr_right_after_a_synchronized_update_starts_takes_effect() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b[?2026h\x1b[31m");
    assert!(terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"\x1b[?2026lx");
    assert!(!terminal.modes().contains(Modes::SYNC));
    assert_eq!(style_at(&terminal, 0, 0).fg, Color::Named(1));
}

// vte: sync_bsu_with_esu
#[test]
fn synchronized_update_ended_and_started_again_at_once() {
    let mut terminal = terminal();
    terminal.advance(b"\x1b[?2026h\x1b[1m");
    terminal.advance(b"\x1b[?2026l\x1b[?2026h\x1b[4m");
    assert!(terminal.modes().contains(Modes::SYNC));
    terminal.advance(b"\x1b[?2026lx");
    assert!(!terminal.modes().contains(Modes::SYNC));
    let attrs = style_at(&terminal, 0, 0).attrs;
    assert!(attrs.contains(Attrs::BOLD) && attrs.contains(Attrs::UNDERLINE));
}
