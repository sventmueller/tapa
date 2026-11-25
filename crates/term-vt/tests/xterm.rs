//! Where the own core follows xterm instead of Alacritty (deviations.md), found by fuzzing against libghostty-vt and
//! checked against xterm's sources; each case as xterm leaves the screen.

use term_vt::cell::CellFlags;
use term_vt::{Size, Terminal};

fn run(columns: usize, lines: usize, bytes: &str) -> Terminal {
    let mut terminal = Terminal::new(Size { columns, lines }, 100);
    terminal.advance(bytes.as_bytes());
    terminal
}

fn text(terminal: &Terminal) -> Vec<String> {
    terminal
        .grid()
        .rows
        .iter()
        .map(|row| row.text().trim_end().to_string())
        .collect()
}

fn cursor(terminal: &Terminal) -> (usize, usize) {
    (terminal.cursor().line, terminal.cursor().column)
}

#[test]
fn erasing_and_editing_end_a_pending_wrap() {
    for sequence in ["\x1b[K", "\x1b[J", "\x1b[1P", "\x1b[1X", "\x1b[1@"] {
        let terminal = run(4, 3, &format!("abcd{sequence}X"));
        assert_eq!(cursor(&terminal), (0, 3), "{sequence:?}");
        assert_eq!(text(&terminal)[1], "", "{sequence:?}");
    }
    for sequence in ["\n", "\x1bD", "\x1bM"] {
        let terminal = run(4, 3, &format!("\x1b[2Habcd{sequence}X"));
        assert!(
            !text(&terminal).iter().any(|line| line.starts_with('X')),
            "{sequence:?} wrapped"
        );
    }
}

#[test]
fn backspace_ends_a_pending_wrap_and_tab_keeps_it() {
    assert_eq!(text(&run(4, 2, "abcd\x08X"))[0], "abXd");
    let tabbed = run(4, 2, "abcd\tX");
    assert_eq!(text(&tabbed), ["abcd", "X"]);
}

#[test]
fn erase_in_line_at_a_pending_wrap_clears_the_last_cell() {
    assert_eq!(text(&run(4, 2, "abcd\x1b[KX"))[0], "abcX");
}

#[test]
fn insert_and_delete_line_return_to_the_left_margin() {
    for sequence in ["\x1b[L", "\x1b[M"] {
        let terminal = run(6, 3, &format!("abc{sequence}"));
        assert_eq!(cursor(&terminal), (0, 0), "{sequence:?}");
    }
}

#[test]
fn erase_above_on_the_second_line_clears_the_first() {
    assert_eq!(text(&run(4, 3, "ab\r\ncd\x1b[1J")), ["", "", ""]);
}

#[test]
fn an_empty_scroll_region_is_ignored() {
    let terminal = run(4, 2, "a\n\x1b[6;7r");
    assert_eq!(cursor(&terminal), (1, 1));
}

#[test]
fn delete_characters_stops_at_the_cursor() {
    assert_eq!(text(&run(4, 2, "A\x1b[7P"))[0], "A");
}

#[test]
fn cursor_up_and_down_stop_at_the_region() {
    let up = run(4, 4, "\x1b[3;4r\x1b[3d\x1b[1A");
    assert_eq!(cursor(&up), (2, 0));
    let down = run(4, 4, "\x1b[1;2r\x1b[5B");
    assert_eq!(cursor(&down), (1, 0));
}

#[test]
fn back_tab_without_a_stop_goes_to_the_first_column() {
    assert_eq!(cursor(&run(8, 2, "\x1b[3g\x1b[3C\x1b[Z")), (0, 0));
}

#[test]
fn origin_mode_homes_on_reset_and_travels_with_the_saved_cursor() {
    assert_eq!(cursor(&run(4, 3, "AA\x1b[?6l")), (0, 0));
    let restored = run(4, 3, "\x1b[?6h\x1b[u\x1b[2;3r");
    assert_eq!(
        cursor(&restored),
        (0, 0),
        "restoring the cursor turned origin mode off again"
    );
}

#[test]
fn the_alternate_screen_restores_and_clears_as_xterm_does() {
    assert_eq!(cursor(&run(4, 2, "0A \x1b[?1049l")), (0, 0));
    assert_eq!(text(&run(4, 2, "\x1b[?1049h A?\x1b[?1049h")), ["", ""]);
}

#[test]
fn alignment_test_homes_the_cursor_and_resets_the_margins() {
    let terminal = run(4, 3, "\x1b[2;3r\x1b[1;31m\x1b[2;2H\x1b#8X");
    assert_eq!(text(&terminal), ["XEEE", "EEEE", "EEEE"]);
    assert_eq!(
        terminal.style(terminal.grid().rows[0].cells[0].style),
        terminal.style(terminal.grid().rows[1].cells[0].style)
    );
}

#[test]
fn a_wide_character_split_by_an_edit_goes_whole() {
    for sequence in ["\x1b[1P", "\x1b[J", "\x1b[K", "\x1b[1X"] {
        let terminal = run(4, 2, &format!("\x1b[1;3H😀\x08{sequence}"));
        let cells = &terminal.grid().rows[0].cells;
        assert!(
            cells.iter().all(|cell| !cell
                .flags
                .intersects(CellFlags(CellFlags::WIDE.0 | CellFlags::WIDE_SPACER.0))),
            "{sequence:?} left half a wide character"
        );
    }
}

#[test]
fn insert_mode_keeps_wide_characters_whole() {
    assert_eq!(text(&run(6, 2, "😀😀😀\x1b[H\x1b[4hx"))[0], "x😀😀");
    assert_eq!(text(&run(7, 2, "😀😀😀\x1b[H\x1b[4h😀"))[0], "😀😀😀");
}

#[test]
fn without_autowrap_the_last_column_still_leaves_a_pending_wrap() {
    let joined = run(4, 2, "\x1b[?7labcde\u{301}");
    assert_eq!(text(&joined)[0], "abce\u{301}");
    let later = run(4, 2, "\x1b[?7labcd\x1b[?7h?");
    assert_eq!(
        text(&later),
        ["abcd", "?"],
        "xterm's dotext sets do_wrap whatever the mode"
    );
    let dropped = run(4, 2, "\x1b[?7l\x1b[1;4H漢\x1b[?7h?");
    assert_eq!(text(&dropped), ["   ?", ""]);
}

#[test]
fn soft_reset_restores_modes_margins_and_rendition_and_keeps_the_screen() {
    let terminal = run(
        6,
        4,
        "ab\x1b[2;3r\x1b[?6h\x1b[4h\x1b[1;31m\x1b(0\x1b[1;2H\x1b7\x1b[!p\x1b[4;5HX\x1b8Y",
    );
    assert_eq!(text(&terminal), ["Yb", "", "", "    X"]);
    let x = &terminal.grid().rows[3].cells[4];
    assert_eq!(terminal.style(x.style), term_vt::style::Style::default());
}
