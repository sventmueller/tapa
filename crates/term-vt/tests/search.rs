//! Behaviour tests of `term_vt::search`, ported from `alacritty_terminal/src/term/search.rs`. Alacritty searches from
//! a point in a direction; `find_all` lists every match of the grid, so each port checks that the match Alacritty
//! finds is among them and covers the same cells.
//!
//! Skipped:
//! - `wrap_around_to_another_end`: `find_all` has no start point, so there is no search that wraps around the grid.
//! - `runtime_cache_error`: tests Alacritty's lazy DFA giving up on a full cache; `regex` has no such failure.
//! - `newline_breaking_semantic`: word selection, not search; ported to `tests/selection.rs`.
//! - `fullwidth_semantic`: needs configurable word separators; ours are fixed and contain no wide character.

use term_vt::search::{Options, compile, find_all};
use term_vt::{Size, Terminal};

type Span = ((u64, usize), (u64, usize));

fn screen(columns: usize, lines: usize, text: &str) -> Terminal {
    let mut terminal = Terminal::new(Size { columns, lines }, 0);
    terminal.advance(text.as_bytes());
    terminal
}

fn regex() -> Options {
    Options {
        regex: true,
        case_sensitive: true,
        whole_word: false,
    }
}

fn spans_with(terminal: &Terminal, pattern: &str, options: Options) -> Vec<Span> {
    let expression = compile(pattern, options).expect("the pattern compiles");
    find_all(terminal.grid(), &expression, usize::MAX)
        .iter()
        .map(|hit| ((hit.start.line, hit.start.column), (hit.end.line, hit.end.column)))
        .collect()
}

fn spans(terminal: &Terminal, pattern: &str) -> Vec<Span> {
    spans_with(terminal, pattern, regex())
}

// From `regex_right` and `regex_left`.
#[test]
fn a_regex_matches_across_a_wrapped_row_but_not_a_hard_line_break() {
    let terminal = screen(9, 5, "testing66\r\nAlacritty123\r\nAlacritty\r\n123");
    assert_eq!(spans(&terminal, "Ala.*123"), [((1, 0), (2, 2))]);
}

// From `nested_regex`.
#[test]
fn a_greedy_regex_stops_at_the_line_break_and_at_an_excluded_character() {
    let terminal = screen(26, 2, "Ala -> Alacritty -> critty\r\ncritty");
    assert_eq!(spans(&terminal, "Ala.*critty"), [((0, 0), (0, 25))]);
    assert_eq!(spans(&terminal, "Ala[^y]*critty"), [((0, 0), (0, 15))]);
}

// From `no_match_right` and `no_match_left`.
#[test]
fn a_pattern_absent_from_the_grid_finds_nothing() {
    let terminal = screen(13, 3, "first line   broken second\r\nthird");
    assert!(spans(&terminal, "nothing").is_empty());
}

// From `include_linebreak_left`.
#[test]
fn a_match_may_end_in_the_last_column_of_a_row_above_another() {
    let terminal = screen(10, 2, "testing123\r\nxxx");
    assert_eq!(spans(&terminal, "te.*123"), [((0, 0), (0, 9))]);
}

// From `include_linebreak_right`.
#[test]
fn a_match_may_fill_the_row_after_a_hard_line_break() {
    let terminal = screen(10, 2, "xxx\r\ntesting123");
    assert_eq!(spans(&terminal, "te.*123"), [((1, 0), (1, 9))]);
}

// From `skip_dead_cell`.
#[test]
fn a_match_may_end_before_the_rest_of_a_word() {
    let terminal = screen(9, 1, "alacritty");
    assert_eq!(spans(&terminal, "alacrit"), [((0, 0), (0, 6))]);
}

// From `reverse_search_dead_recovery`.
#[test]
fn a_match_may_end_inside_a_run_of_the_same_character() {
    let terminal = screen(10, 1, "zooo lense");
    assert_eq!(spans(&terminal, "zoo"), [((0, 0), (0, 2))]);
}

// From `multibyte_unicode`.
#[test]
fn multibyte_characters_count_as_one_cell_each() {
    let terminal = screen(12, 1, "testвосибing");
    assert_eq!(spans(&terminal, "te.*ing"), [((0, 0), (0, 11))]);
}

// From `end_on_multibyte_unicode`.
#[test]
fn a_match_may_end_on_a_multibyte_character() {
    let terminal = screen(9, 1, "testвосиб");
    assert_eq!(spans(&terminal, "te.*и"), [((0, 0), (0, 7))]);
}

// From `fullwidth`.
#[test]
fn a_match_ending_on_a_wide_character_covers_its_spacer() {
    let terminal = screen(6, 1, "a🦇x🦇");
    assert_eq!(spans(&terminal, "[^ ]*"), [((0, 0), (0, 5))]);
}

// From `singlecell_fullwidth`.
#[test]
fn a_lone_wide_character_matches_both_its_cells() {
    let terminal = screen(2, 1, "🦇");
    assert_eq!(spans(&terminal, "🦇"), [((0, 0), (0, 1))]);
}

// From `end_on_fullwidth`.
#[test]
fn a_wide_character_at_the_end_of_a_row_is_found_whole_and_nothing_else_is() {
    let terminal = screen(6, 1, "jarr🦇");
    assert!(spans(&terminal, "x").is_empty());
    assert_eq!(spans(&terminal, "jarr🦇"), [((0, 0), (0, 5))]);
}

// From `wrapping`.
#[test]
fn matches_on_rows_split_by_a_hard_line_break_are_separate() {
    let terminal = screen(3, 2, "xxx\r\nxxx");
    assert_eq!(spans(&terminal, "xxx"), [((0, 0), (0, 2)), ((1, 0), (1, 2))]);
}

// From `wrapping_into_fullwidth`.
#[test]
fn a_match_may_start_or_end_on_a_wide_character_at_a_row_edge() {
    let terminal = screen(4, 2, "🦇xx\r\nxx🦇");
    assert_eq!(spans(&terminal, "🦇x"), [((0, 0), (0, 2))]);
    assert_eq!(spans(&terminal, "x🦇"), [((1, 1), (1, 3))]);
}

// From `multiline`.
#[test]
fn a_pattern_that_may_be_empty_finds_the_words_of_every_line() {
    let terminal = screen(5, 2, "test \r\ntest");
    assert_eq!(spans(&terminal, "[a-z]*"), [((0, 0), (0, 3)), ((1, 0), (1, 3))]);
}

// From `empty_match`.
#[test]
fn empty_matches_are_dropped() {
    let terminal = screen(5, 1, " abc ");
    assert_eq!(spans(&terminal, "[a-z]*"), [((0, 1), (0, 3))]);
}

// From `empty_match_multibyte`.
#[test]
fn empty_matches_next_to_a_multibyte_character_are_dropped() {
    let terminal = screen(2, 1, " ↑");
    assert!(spans(&terminal, "[a-z]*").is_empty());
}

// From `empty_match_multiline`.
#[test]
fn blanks_of_a_wrapped_row_separate_matches() {
    let terminal = screen(13, 2, "abc          xxx");
    assert_eq!(spans(&terminal, "[a-z]*"), [((0, 0), (0, 2)), ((1, 0), (1, 2))]);
}

// From `leading_spacer`.
#[test]
fn a_wide_character_pushed_to_the_next_row_matches_across_the_leading_spacer() {
    let terminal = screen(4, 2, "xxx🦇xx");
    // Alacritty starts this match on the leading spacer at (0, 3); see tests/ref/deviations.md.
    assert_eq!(spans(&terminal, "🦇x"), [((1, 0), (1, 2))]);
    assert_eq!(spans(&terminal, "x🦇"), [((0, 2), (1, 1))]);
}

// From `wide_without_spacer`.
#[test]
fn a_wide_character_without_its_spacer_does_not_break_the_search() {
    let mut terminal = screen(2, 2, "x");
    let row = &mut terminal.grid_mut().rows[0];
    row.cells[1].c = '字';
    row.cells[1].flags = term_vt::cell::CellFlags::WIDE;
    assert!(spans(&terminal, "test").is_empty());
    assert_eq!(spans(&terminal, "x字"), [((0, 0), (0, 1))]);
}

// From `nfa_compile_error`.
#[test]
fn a_pattern_too_big_to_compile_is_an_error() {
    assert!(compile("[0-9A-Za-z]{9999999}", regex()).is_err());
}

// From `greed_is_good`.
#[test]
fn the_leftmost_alternative_that_matches_wins() {
    let terminal = screen(18, 1, "https://github.com");
    assert_eq!(spans(&terminal, "/github.com|https://github.com"), [((0, 0), (0, 17))]);
}

// From `anchored_empty`.
#[test]
fn an_empty_first_alternative_hides_a_longer_one_at_the_same_place() {
    let terminal = screen(4, 1, "rust");
    // Alacritty finds "rust" here; see tests/ref/deviations.md.
    assert!(spans(&terminal, ";*|rust").is_empty());
    assert_eq!(spans(&terminal, "rust|;*"), [((0, 0), (0, 3))]);
}

// From `inline_word_search`.
#[test]
fn a_word_split_by_a_wrap_is_found_between_the_others() {
    let terminal = screen(21, 2, "word word word word word word word word");
    let found = spans(&terminal, "word");
    assert_eq!(found.len(), 8);
    assert_eq!(found[4], ((0, 20), (1, 2)));
}

// From `fullwidth_across_lines`.
#[test]
fn wide_characters_on_both_sides_of_a_wrap_match_together() {
    let terminal = screen(3, 2, "a🦇🦇b");
    assert_eq!(spans(&terminal, "🦇🦇"), [((0, 1), (1, 1))]);
}

// From `fullwidth_into_halfwidth_across_lines`.
#[test]
fn a_wide_character_before_a_wrap_matches_with_the_next_row() {
    let terminal = screen(3, 2, "a🦇xab");
    assert_eq!(spans(&terminal, "🦇x"), [((0, 1), (1, 0))]);
}

// From `no_spacer_fullwidth_linewrap`.
#[test]
fn a_wide_character_stored_without_flags_matches_as_one_cell() {
    let mut terminal = screen(3, 2, "abYxab");
    terminal.grid_mut().rows[0].cells[2].c = '🦇';
    assert_eq!(spans(&terminal, "🦇x"), [((0, 2), (1, 0))]);
}

// No Alacritty counterpart: Alacritty searches only a cell's first character.
#[test]
fn zero_width_characters_belong_to_the_cell_before_them() {
    let terminal = screen(4, 1, "ae\u{301}xe");
    assert_eq!(spans(&terminal, "e\u{301}"), [((0, 1), (0, 1))]);
    assert_eq!(spans(&terminal, "x"), [((0, 2), (0, 2))]);
    assert_eq!(spans(&terminal, "e$"), [((0, 3), (0, 3))]);
}

// No Alacritty counterpart: Alacritty has smart case and only regular expressions.
#[test]
fn plain_text_case_and_whole_words_follow_the_options() {
    let terminal = screen(12, 1, "a.c abc ABC");
    let plain = Options::default();
    assert_eq!(spans_with(&terminal, "a.c", plain), [((0, 0), (0, 2))]);
    assert_eq!(spans_with(&terminal, "abc", plain).len(), 2);
    let exact = Options {
        case_sensitive: true,
        ..plain
    };
    assert_eq!(spans_with(&terminal, "ABC", exact), [((0, 8), (0, 10))]);
    let words = Options {
        whole_word: true,
        ..plain
    };
    let terminal = screen(12, 1, "ab abc abcd");
    assert_eq!(spans_with(&terminal, "abc", words), [((0, 3), (0, 5))]);
}

// No Alacritty counterpart: the anchors of a logical line.
#[test]
fn anchors_mark_the_ends_of_a_logical_line_not_of_a_row() {
    let terminal = screen(3, 3, "abcdef\r\nghi");
    assert_eq!(spans(&terminal, "^."), [((0, 0), (0, 0)), ((2, 0), (2, 0))]);
    assert_eq!(spans(&terminal, ".$"), [((1, 2), (1, 2)), ((2, 2), (2, 2))]);
}
