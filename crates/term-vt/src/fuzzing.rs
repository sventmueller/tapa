//! Entry points for the fuzz targets in `fuzz/`.

use crate::scan::Scanner;

/// Feeds `bytes` to the scanner in pieces of the size the first byte picks, and checks that the pieces find exactly
/// the marks the whole output finds at once.
pub fn scan(bytes: &[u8]) {
    let Some((&first, rest)) = bytes.split_first() else {
        return;
    };
    let mut pieces = Scanner::new();
    let mut found = Vec::new();
    for piece in rest.chunks(usize::from(first % 32) + 1) {
        pieces.feed(piece);
        found.extend(pieces.take_events());
    }
    let mut whole = Scanner::new();
    whole.feed(rest);
    assert_eq!(found, whole.take_events(), "pieces of {}", usize::from(first % 32) + 1);
}

/// Feeds `bytes` to a terminal whose size the first two bytes pick, cut where the third says and resized midway, so
/// that only a panic, a hang or a runaway allocation fails (F183).
pub fn terminal(bytes: &[u8]) {
    let [columns, lines, cut, rest @ ..] = bytes else {
        return;
    };
    let size = |c: u8, l: u8| crate::Size {
        columns: usize::from(c % 200) + 1,
        lines: usize::from(l % 60) + 1,
    };
    let mut terminal = crate::Terminal::new(size(*columns, *lines), 200);
    terminal.allow_clipboard_read(true);
    let cut = usize::from(*cut).min(rest.len());
    terminal.advance(&rest[..cut]);
    terminal.resize(size(*lines, *columns));
    terminal.advance(&rest[cut..]);
    let _ = terminal.take_events();
}
