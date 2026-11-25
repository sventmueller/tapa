//! Cases from esctest and vttest runs in the app (E11 point 4), one for each fault they showed in the own core;
//! `tests/ref/esctest.md` records the runs.

use term_vt::{Event, Size, Terminal};

fn terminal(columns: usize, lines: usize) -> Terminal {
    Terminal::new(Size { columns, lines }, 0)
}

/// The replies to `input`, joined.
fn replies(terminal: &mut Terminal, input: &[u8]) -> String {
    terminal.advance(input);
    terminal
        .take_events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Reply(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            _ => None,
        })
        .collect()
}

/// The checksum DECRQCRA reports for one cell.
fn checksum_at(terminal: &mut Terminal, line: usize, column: usize) -> String {
    let reply = replies(
        terminal,
        format!("\x1b[7;0;{line};{column};{line};{column}*y").as_bytes(),
    );
    reply
        .strip_prefix("\x1bP7!~")
        .and_then(|rest| rest.strip_suffix("\x1b\\"))
        .unwrap_or(&reply)
        .to_string()
}

#[test]
fn text_area_size_in_characters_is_reported() {
    let mut term = terminal(80, 25);
    assert_eq!(replies(&mut term, b"\x1b[18t"), "\x1b[8;25;80t");
}

#[test]
fn rectangle_checksum_sums_characters_and_counts_empty_cells_as_blanks() {
    let mut term = terminal(10, 4);
    term.advance(b"ab\r\ncd");
    assert_eq!(
        replies(&mut term, b"\x1b[1;0;1;1;2;2*y"),
        format!("\x1bP1!~{:04X}\x1b\\", 0x61 + 0x62 + 0x63 + 0x64)
    );
    assert_eq!(checksum_at(&mut term, 1, 3), "0020");
    assert_eq!(
        replies(&mut term, b"\x1b[2*y"),
        format!("\x1bP2!~{:04X}\x1b\\", (40 - 4) * 32 + 0x61 + 0x62 + 0x63 + 0x64)
    );
}

#[test]
fn rectangle_checksum_counts_from_the_region_in_origin_mode() {
    let mut term = terminal(10, 6);
    term.advance(b"\x1b[3;5r\x1b[3;1Hx\x1b[?6h");
    assert_eq!(checksum_at(&mut term, 1, 1), "0078");
}

#[test]
fn modes_are_reported() {
    let mut term = terminal(10, 4);
    assert_eq!(replies(&mut term, b"\x1b[4$p"), "\x1b[4;2$y");
    assert_eq!(replies(&mut term, b"\x1b[4h\x1b[4$p"), "\x1b[4;1$y");
    assert_eq!(replies(&mut term, b"\x1b[?7$p"), "\x1b[?7;1$y");
    assert_eq!(replies(&mut term, b"\x1b[?6$p"), "\x1b[?6;2$y");
    assert_eq!(replies(&mut term, b"\x1b[?9999$p"), "\x1b[?9999;0$y");
}

#[test]
fn decst8c_sets_a_tab_stop_every_eight_columns() {
    let mut term = terminal(20, 2);
    term.advance(b"\x1b[3g\x1b[?5W\tx");
    assert_eq!(term.grid().rows[0].cells[8].c, 'x');
}

#[test]
fn deleting_characters_at_the_line_end_keeps_those_before_the_cursor() {
    let mut term = terminal(80, 24);
    term.advance(b"\x1b[2;1H");
    term.advance(&[b'B'; 80]);
    term.advance(b"\x1b[2;78H\x1b[2P");
    let row: String = term.grid().rows[1].cells.iter().map(|c| c.c).collect();
    assert_eq!(row.trim_end().len(), 78, "{row:?}");
}
