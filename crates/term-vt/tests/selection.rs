//! Behaviour tests of `term_vt::selection`, ported from `alacritty_terminal/src/selection.rs`, plus the selection
//! tests of `alacritty_terminal/src/term/mod.rs` and `term/search.rs` that check ranges and copied text. Alacritty's
//! `rotate` moves a selection with scrolled rows; ours is anchored in global line numbers, so those ports scroll the
//! terminal instead and check that the selection stays with its rows.
//!
//! Skipped:
//! - `rotate_in_region_up`, `rotate_in_region_down`, `rotate_in_region_up_block`: scrolling inside a scroll region
//!   does not move global line numbers, so the selection stays where it is; see tests/ref/deviations.md.
//! - `range_intersection`: tests `Selection::intersects_range`, which has no counterpart here.

use term_vt::selection::{Anchor, Kind, Selection, Side};
use term_vt::{Size, Terminal};

type Range = ((u64, usize), (u64, usize));

fn screen(columns: usize, lines: usize, text: &str) -> Terminal {
    let mut terminal = Terminal::new(Size { columns, lines }, 0);
    terminal.advance(text.as_bytes());
    terminal
}

fn at(line: u64, column: usize) -> Anchor {
    Anchor { line, column }
}

fn select(kind: Kind, start: (Anchor, Side), end: (Anchor, Side)) -> Selection {
    Selection { kind, start, end }
}

fn range(terminal: &Terminal, selection: &Selection) -> Option<Range> {
    selection
        .range(terminal.grid())
        .map(|(first, last)| ((first.line, first.column), (last.line, last.column)))
}

fn text(terminal: &Terminal, selection: &Selection) -> Option<String> {
    selection.text(terminal.grid())
}

/// A 5 × 10 terminal with a history, a selection of `kind` from line 9 to line 4, and four rows scrolled up.
fn scrolled(kind: Kind, start: (Anchor, Side), end: (Anchor, Side)) -> (Terminal, Option<Range>) {
    let mut terminal = Terminal::new(Size { columns: 5, lines: 10 }, 100);
    let selection = select(kind, start, end);
    terminal.advance(b"\x1b[10;1H\n\n\n\n");
    let range = range(&terminal, &selection);
    let grid = terminal.grid();
    let view = range.map(|((first, _), (last, _))| (grid.view_of_global(first), grid.view_of_global(last)));
    assert_eq!(view, Some((Some(0), Some(5))), "the selection moves up with its rows");
    (terminal, range)
}

// From `single_cell_left_to_right`.
#[test]
fn going_from_the_left_to_the_right_half_of_a_cell_selects_it() {
    let terminal = screen(2, 1, "");
    let selection = select(Kind::Simple, (at(0, 0), Side::Left), (at(0, 0), Side::Right));
    assert_eq!(range(&terminal, &selection), Some(((0, 0), (0, 0))));
}

// From `single_cell_right_to_left`.
#[test]
fn going_from_the_right_to_the_left_half_of_a_cell_selects_it() {
    let terminal = screen(2, 1, "");
    let selection = select(Kind::Simple, (at(0, 0), Side::Right), (at(0, 0), Side::Left));
    assert_eq!(range(&terminal, &selection), Some(((0, 0), (0, 0))));
}

// From `between_adjacent_cells_left_to_right`.
#[test]
fn going_from_a_right_half_to_the_next_left_half_selects_nothing() {
    let terminal = screen(2, 1, "");
    let selection = select(Kind::Simple, (at(0, 0), Side::Right), (at(0, 1), Side::Left));
    assert_eq!(range(&terminal, &selection), None);
}

// From `between_adjacent_cells_right_to_left`.
#[test]
fn going_from_a_left_half_to_the_previous_right_half_selects_nothing() {
    let terminal = screen(2, 1, "");
    let selection = select(Kind::Simple, (at(0, 1), Side::Left), (at(0, 0), Side::Right));
    assert_eq!(range(&terminal, &selection), None);
}

// From `across_adjacent_lines_upward_final_cell_exclusive`.
#[test]
fn a_selection_starting_on_a_right_half_leaves_out_that_cell() {
    let terminal = screen(5, 2, "");
    let selection = select(Kind::Simple, (at(0, 1), Side::Right), (at(1, 1), Side::Right));
    assert_eq!(range(&terminal, &selection), Some(((0, 2), (1, 1))));
}

// From `selection_bigger_then_smaller`.
#[test]
fn moving_the_end_past_the_start_orders_the_range() {
    let terminal = screen(5, 2, "");
    let mut selection = select(Kind::Simple, (at(1, 1), Side::Right), (at(0, 1), Side::Right));
    selection.end = (at(0, 0), Side::Right);
    assert_eq!(range(&terminal, &selection), Some(((0, 1), (1, 1))));
}

// From `line_selection`.
#[test]
fn a_line_selection_covers_whole_rows_and_stays_with_them() {
    let (_, range) = scrolled(Kind::Line, (at(9, 1), Side::Left), (at(4, 1), Side::Right));
    assert_eq!(range, Some(((4, 0), (9, 4))));
}

// From `semantic_selection`.
#[test]
fn a_word_selection_on_blanks_keeps_its_ends_and_stays_with_its_rows() {
    let (_, range) = scrolled(Kind::Word, (at(9, 3), Side::Left), (at(4, 1), Side::Right));
    assert_eq!(range, Some(((4, 1), (9, 3))));
}

// From `simple_selection`.
#[test]
fn a_simple_selection_applies_its_sides_and_stays_with_its_rows() {
    let (_, range) = scrolled(Kind::Simple, (at(9, 3), Side::Right), (at(4, 1), Side::Right));
    assert_eq!(range, Some(((4, 2), (9, 3))));
}

// From `block_selection`.
#[test]
fn a_block_selection_applies_its_sides_to_the_columns_and_stays_with_its_rows() {
    let (_, range) = scrolled(Kind::Block, (at(9, 3), Side::Right), (at(4, 1), Side::Right));
    assert_eq!(range, Some(((4, 2), (9, 3))));
}

// From `simple_is_empty`.
#[test]
fn a_simple_selection_is_empty_until_it_covers_a_whole_cell() {
    let terminal = screen(2, 2, "");
    let mut selection = select(Kind::Simple, (at(1, 0), Side::Right), (at(1, 0), Side::Right));
    assert_eq!(range(&terminal, &selection), None);
    selection.end = (at(1, 1), Side::Left);
    assert_eq!(range(&terminal, &selection), None);
    selection.end = (at(0, 0), Side::Right);
    assert!(range(&terminal, &selection).is_some());
}

// From `block_is_empty`.
#[test]
fn a_block_selection_is_empty_until_it_covers_a_whole_column() {
    let terminal = screen(2, 2, "");
    let mut selection = select(Kind::Block, (at(1, 0), Side::Right), (at(1, 0), Side::Right));
    let steps = [
        ((1, 1, Side::Left), false),
        ((1, 1, Side::Right), true),
        ((0, 0, Side::Right), false),
        ((0, 1, Side::Left), false),
        ((0, 1, Side::Right), true),
    ];
    assert_eq!(range(&terminal, &selection), None);
    for ((line, column, side), selected) in steps {
        selection.end = (at(line, column), side);
        assert_eq!(
            range(&terminal, &selection).is_some(),
            selected,
            "{line}, {column}, {side:?}"
        );
    }
}

// From `term/mod.rs` `simple_selection_works`.
#[test]
fn copied_text_keeps_empty_lines_and_the_blanks_of_a_wrapped_row() {
    let terminal = screen(5, 5, "\"aaa\"\r\n\r\n aaa  aaa\"");
    let selection = select(Kind::Simple, (at(0, 0), Side::Left), (at(2, 4), Side::Right));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("\"aaa\"\n\n aaa "));
    let selection = select(Kind::Simple, (at(2, 0), Side::Left), (at(3, 4), Side::Right));
    assert_eq!(text(&terminal, &selection).as_deref(), Some(" aaa  aaa\""));
}

// From `term/mod.rs` `semantic_selection_works`.
#[test]
fn a_word_selection_stops_at_quotes_and_follows_a_wrap() {
    let terminal = screen(5, 3, "\"aa\"aaa\"aa");
    let word = |line, column| {
        text(
            &terminal,
            &select(
                Kind::Word,
                (at(line, column), Side::Left),
                (at(line, column), Side::Left),
            ),
        )
    };
    assert_eq!(word(0, 1).as_deref(), Some("aa"));
    assert_eq!(word(0, 4).as_deref(), Some("aaa"));
    assert_eq!(word(1, 1).as_deref(), Some("aaa"));
}

// From `term/mod.rs` `line_selection_works`.
#[test]
fn a_line_selection_copies_the_row_with_a_newline() {
    let terminal = screen(5, 1, "\"aa\"a");
    let selection = select(Kind::Line, (at(0, 3), Side::Left), (at(0, 3), Side::Left));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("\"aa\"a\n"));
}

// From `term/mod.rs` `block_selection_works`.
#[test]
fn a_block_selection_copies_its_columns_line_by_line_without_trailing_blanks() {
    let terminal = screen(5, 5, "\r\n\"aaa\"\r\n\"a a\"\"aaa");
    let mut selection = select(Kind::Block, (at(0, 3), Side::Left), (at(3, 3), Side::Right));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("\na\na\na"));
    selection.end = (at(3, 0), Side::Left);
    assert_eq!(text(&terminal, &selection).as_deref(), Some("\n\"aa\n\"a\n\"aa"));
    selection.end = (at(3, 4), Side::Right);
    assert_eq!(text(&terminal, &selection).as_deref(), Some("\na\"\na\"\na"));
}

// From `term/search.rs` `newline_breaking_semantic`.
#[test]
fn a_word_selection_does_not_cross_a_hard_line_break() {
    let terminal = screen(8, 2, "test abc\r\ndef test");
    let word = |line, column| {
        range(
            &terminal,
            &select(
                Kind::Word,
                (at(line, column), Side::Left),
                (at(line, column), Side::Left),
            ),
        )
    };
    assert_eq!(word(0, 7), Some(((0, 5), (0, 7))));
    assert_eq!(word(1, 0), Some(((1, 0), (1, 2))));
}

// No Alacritty test; the behaviour of `line_search_left` and `line_search_right` in `term/search.rs`.
#[test]
fn a_line_selection_covers_every_row_of_a_wrapped_line() {
    let terminal = screen(3, 3, "abcdefg");
    let selection = select(Kind::Line, (at(1, 1), Side::Left), (at(1, 1), Side::Left));
    assert_eq!(range(&terminal, &selection), Some(((0, 0), (2, 2))));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("abcdefg\n"));
}

// No Alacritty test; the wide character rule of `line_to_string` in `term/mod.rs`.
#[test]
fn a_selection_starting_on_the_spacer_of_a_wide_character_copies_the_character() {
    let terminal = screen(5, 1, "a字b");
    let selection = select(Kind::Simple, (at(0, 2), Side::Left), (at(0, 3), Side::Right));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("字b"));
    let selection = select(Kind::Simple, (at(0, 0), Side::Left), (at(0, 2), Side::Right));
    assert_eq!(text(&terminal, &selection).as_deref(), Some("a字"));
}
