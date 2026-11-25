//! What the keyboard's copy mode (F111) reads and does.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

#[allow(
    dead_code,
    reason = "the harness is shared with the session tests, which use the rest of it"
)]
mod common;

use common::{Harness, command, temporary};
use term_session::{SelectionKind, Size};

fn printed(name: &str) -> Harness {
    let dir = temporary(name);
    let mut options = command(
        "/bin/sh",
        &[
            "-c",
            "i=0; while [ $i -lt 30 ]; do i=$((i+1)); echo row$i; done; printf '日本 end'; sleep 30",
        ],
        &dir,
    );
    options.size = Size {
        columns: 20,
        lines: 5,
        ..Size::default()
    };
    let mut harness = Harness::start(options, &dir);
    assert!(harness.wait_until(|h| h.screen_has("end")));
    harness
}

#[test]
fn copy_mode_reads_scrolls_and_selects() {
    let harness = printed("copy");
    let session = &harness.session;
    let text = session.rows_text();
    let at = text.rows.iter().position(|row| row == "row1").unwrap();
    assert_eq!(text.rows[at + 29], "row30");
    let last = text.rows.iter().position(|row| row == "日本 end").unwrap();
    assert_eq!(text.cells[&last], [0, 2, 4, 5, 6, 7]);
    assert_eq!(text.cursor.0, last);

    let first = text.first + at as u64;
    assert!(session.scroll_to_row(first));
    let view = session.rows_view();
    assert_eq!(view.line_of(first), Some(0));
    assert!(!session.scroll_to_row(first + 2));

    session.select_rows(SelectionKind::Simple, (first, 1), (first + 1, 2));
    assert_eq!(session.selected_text().unwrap(), "ow1\nrow");
    session.select_rows(SelectionKind::Line, (first + 2, 3), (first + 3, 0));
    assert_eq!(session.selected_text().unwrap(), "row3\nrow4\n");
    session.select_rows(SelectionKind::Block, (first, 1), (first + 2, 2));
    assert_eq!(session.selected_text().unwrap(), "ow\now\now");
}
