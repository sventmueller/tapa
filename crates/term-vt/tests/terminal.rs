//! The terminal and grid tests of `alacritty_terminal` (`term/mod.rs`, `grid/tests.rs`) on the own core (E4, E11),
//! driven through escape sequences where Alacritty calls its handler directly.
//!
//! Not taken over: the vi-mode tests (the own core has no vi cursor), the damage tests (it tracks no damage; the
//! renderer compares lines), `grid_serde` and `parse_cargo_version` (no such feature), the `*_reflow_disabled` tests
//! (the own core always reflows) and `test_iter`/`accurate_size_hint` (no such iterator); the selection and search
//! tests live in `selection.rs` and `search.rs`.

use term_vt::grid::Grid;
use term_vt::style::StyleId;
use term_vt::{Size, Terminal};

fn terminal(columns: usize, lines: usize, history: usize) -> Terminal {
    Terminal::new(Size { columns, lines }, history)
}

/// Every row from the top of the history down, as its characters and whether it wraps.
fn all_rows(terminal: &Terminal) -> Vec<(String, bool)> {
    let grid = terminal.grid();
    grid.history
        .iter()
        .chain(grid.rows.iter().map(std::borrow::Cow::Borrowed))
        .map(|row| (row.cells.iter().map(|cell| cell.c).collect(), row.wrapped()))
        .collect()
}

fn rows(expected: &[(&str, bool)]) -> Vec<(String, bool)> {
    expected
        .iter()
        .map(|&(text, wraps)| (text.to_string(), wraps))
        .collect()
}

fn newlines(terminal: &mut Terminal, count: usize) {
    terminal.advance(&b"\n".repeat(count));
}

#[test]
fn scroll_display_page_up() {
    let mut term = terminal(5, 10, 10_000);
    newlines(&mut term, 20);
    let grid = term.grid_mut();
    grid.scroll_view(10);
    assert_eq!(grid.display_offset, 10);
    grid.scroll_view(10);
    assert_eq!(grid.display_offset, 11);
    grid.scroll_view(10);
    assert_eq!(grid.display_offset, 11);
}

#[test]
fn scroll_display_page_down() {
    let mut term = terminal(5, 10, 10_000);
    newlines(&mut term, 20);
    let grid = term.grid_mut();
    grid.scroll_view(isize::MAX);
    grid.scroll_view(-10);
    assert_eq!(grid.display_offset, 1);
    grid.scroll_view(-10);
    assert_eq!(grid.display_offset, 0);
    grid.scroll_view(-10);
    assert_eq!(grid.display_offset, 0);
}

#[test]
fn input_line_drawing_character() {
    let mut term = terminal(17, 7, 10_000);
    term.advance(b"\x1b(0a");
    assert_eq!(term.grid().rows[0].cells[0].c, '▒');
}

#[test]
fn clearing_viewport_keeps_history_position() {
    let mut term = terminal(10, 20, 10_000);
    newlines(&mut term, 29);
    term.grid_mut().scroll_view(isize::MAX);
    assert_eq!(term.grid().display_offset, 10);
    term.advance(b"\x1b[2J");
    assert_eq!(term.grid().display_offset, 10);
}

#[test]
fn clearing_scrollback_resets_display_offset() {
    let mut term = terminal(10, 20, 10_000);
    newlines(&mut term, 29);
    term.grid_mut().scroll_view(isize::MAX);
    assert_eq!(term.grid().display_offset, 10);
    term.advance(b"\x1b[3J");
    assert_eq!(term.grid().display_offset, 0);
}

#[test]
fn clear_saved_lines() {
    let mut term = terminal(17, 7, 10_000);
    term.grid_mut().scroll_up(0..1, 1, StyleId(0));
    assert_eq!(term.grid().history.len(), 1);
    term.advance(b"\x1b[3J");
    assert!(term.grid().history.is_empty());
}

#[test]
fn grow_lines_updates_active_cursor_pos() {
    let mut term = terminal(100, 10, 10_000);
    newlines(&mut term, 19);
    assert_eq!(term.grid().history.len(), 10);
    assert_eq!(term.cursor().line, 9);
    term.resize(Size {
        columns: 100,
        lines: 30,
    });
    assert_eq!(term.grid().history.len(), 0);
    assert_eq!((term.cursor().line, term.cursor().column), (19, 0));
}

#[test]
fn grow_lines_updates_inactive_cursor_pos() {
    let mut term = terminal(100, 10, 10_000);
    newlines(&mut term, 19);
    assert_eq!(term.grid().history.len(), 10);
    assert_eq!(term.cursor().line, 9);
    term.advance(b"\x1b[?1049h");
    term.resize(Size {
        columns: 100,
        lines: 30,
    });
    term.advance(b"\x1b[?1049l");
    assert_eq!(term.grid().history.len(), 0);
    assert_eq!((term.cursor().line, term.cursor().column), (19, 0));
}

#[test]
fn shrink_lines_updates_active_cursor_pos() {
    let mut term = terminal(100, 10, 10_000);
    newlines(&mut term, 19);
    term.resize(Size { columns: 100, lines: 5 });
    assert_eq!(term.grid().history.len(), 15);
    assert_eq!((term.cursor().line, term.cursor().column), (4, 0));
}

#[test]
fn shrink_lines_updates_inactive_cursor_pos() {
    let mut term = terminal(100, 10, 10_000);
    newlines(&mut term, 19);
    term.advance(b"\x1b[?1049h");
    term.resize(Size { columns: 100, lines: 5 });
    term.advance(b"\x1b[?1049l");
    assert_eq!(term.grid().history.len(), 15);
    assert_eq!((term.cursor().line, term.cursor().column), (4, 0));
}

#[test]
fn window_title() {
    let mut term = terminal(17, 7, 10_000);
    assert_eq!(term.title(), None);

    term.advance(b"\x1b]2;Test\x07");
    assert_eq!(term.title(), Some("Test"));
    term.advance(b"\x1b[22t\x1b]2;Next\x07");
    assert_eq!(term.title(), Some("Next"));
    term.advance(b"\x1b[23t");
    assert_eq!(term.title(), Some("Test"));
    term.advance(b"\x1b]2;Kept\x07\x1b[23t");
    assert_eq!(term.title(), Some("Kept"), "the stack is empty again");

    // The stack keeps 4096 titles and drops the oldest.
    for n in 0..4097 {
        term.advance(format!("\x1b]2;{n}\x07\x1b[22t").as_bytes());
    }
    term.advance(&b"\x1b[23t".repeat(4097));
    assert_eq!(term.title(), Some("1"));

    term.advance(b"\x1b[22t\x1bc");
    assert_eq!(term.title(), None);
    term.advance(b"\x1b]2;Kept\x07\x1b[23t");
    assert_eq!(term.title(), Some("Kept"), "a reset empties the stack");

    let mut term = terminal(17, 7, 10_000);
    term.advance(b"\x1b[22t\x1b]2;Test\x07\x1b[23t");
    assert_eq!(term.title(), None);
}

#[test]
fn grid_scroll_up() {
    let mut grid = Grid::new(10, 1, 0);
    for (line, c) in ('0'..='9').enumerate() {
        grid.rows[line].cells[0].c = c;
        grid.rows[line].touch(1);
    }
    grid.scroll_up(0..10, 2, StyleId(0));
    let column: String = grid.rows.iter().map(|row| row.cells[0].c).collect();
    assert_eq!(column, "23456789  ");
}

#[test]
fn grid_scroll_down() {
    let mut grid = Grid::new(10, 1, 0);
    for (line, c) in ('0'..='9').enumerate() {
        grid.rows[line].cells[0].c = c;
        grid.rows[line].touch(1);
    }
    grid.scroll_down(0..10, 2, StyleId(0));
    let column: String = grid.rows.iter().map(|row| row.cells[0].c).collect();
    assert_eq!(column, "  01234567");
}

#[test]
fn grid_scroll_down_keeps_history() {
    let mut grid = Grid::new(10, 1, 1);
    for (line, c) in ('0'..='9').enumerate() {
        grid.rows[line].cells[0].c = c;
        grid.rows[line].touch(1);
    }
    grid.scroll_up(0..10, 1, StyleId(0));
    grid.scroll_down(0..10, 2, StyleId(0));
    let column: String = grid.rows.iter().map(|row| row.cells[0].c).collect();
    assert_eq!(column, "  12345678");
    assert_eq!(grid.history.get(0).unwrap().cells[0].c, '0');
}

/// Writes `text` from the top left and puts the cursor back there, as the grid tests' cursor never moves.
fn written(columns: usize, lines: usize, history: usize, text: &str) -> Terminal {
    let mut term = terminal(columns, lines, history);
    term.advance(text.as_bytes());
    term.advance(b"\x1b[H");
    term
}

#[test]
fn shrink_reflow() {
    let mut term = written(5, 1, 2, "12345");
    term.resize(Size { columns: 2, lines: 1 });
    assert_eq!(all_rows(&term), rows(&[("12", true), ("34", true), ("5 ", false)]));
}

#[test]
fn shrink_reflow_twice() {
    let mut term = written(5, 1, 2, "12345");
    term.resize(Size { columns: 4, lines: 1 });
    term.resize(Size { columns: 2, lines: 1 });
    assert_eq!(all_rows(&term), rows(&[("12", true), ("34", true), ("5 ", false)]));
}

#[test]
fn shrink_reflow_empty_cell_inside_line() {
    let mut term = written(5, 1, 3, "1 34");
    term.resize(Size { columns: 2, lines: 1 });
    assert_eq!(all_rows(&term), rows(&[("1 ", true), ("34", false)]));
}

#[test]
fn one_column_is_two_so_a_wide_character_still_fits() {
    let mut term = Terminal::new(Size { columns: 5, lines: 3 }, 50);
    term.advance("漢".as_bytes());
    term.resize(Size { columns: 1, lines: 3 });
    assert_eq!(term.size().columns, 2);
}

#[test]
fn grow_reflow() {
    let mut term = written(2, 2, 0, "123");
    term.resize(Size { columns: 3, lines: 2 });
    assert_eq!(all_rows(&term), rows(&[("123", false), ("   ", false)]));
}

#[test]
fn grow_reflow_multiline() {
    let mut term = written(2, 3, 0, "123456");
    term.resize(Size { columns: 6, lines: 3 });
    assert_eq!(
        all_rows(&term),
        rows(&[("123456", false), ("      ", false), ("      ", false)])
    );
}
