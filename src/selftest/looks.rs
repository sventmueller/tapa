//! The self-test's step for how the sidebar looks while it has the keyboard: `emphasize:<on|off>` draws the selected
//! row of every table in the front window as the emphasized selection of a key window, which a self-test running
//! behind another app never gets on its own.

use objc2_app_kit::{NSTableView, NSView};

use crate::state::app;

/// Runs `step` if it is `emphasize:`; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some(on) = step.strip_prefix("emphasize:") else {
        return false;
    };
    let Some(content) = app()
        .front()
        .and_then(|window| window.front_ns_window())
        .and_then(|window| window.contentView())
    else {
        return true;
    };
    let mut rows = 0;
    emphasize(&content, on == "on", &mut rows);
    println!("emphasize {on}: {rows} selected rows");
    true
}

fn emphasize(view: &NSView, on: bool, rows: &mut usize) {
    if let Some(table) = view.downcast_ref::<NSTableView>() {
        for row in 0..table.numberOfRows() {
            if !table.isRowSelected(row) {
                continue;
            }
            if let Some(row_view) = table.rowViewAtRow_makeIfNecessary(row, false) {
                row_view.setEmphasized(on);
                *rows += 1;
            }
        }
    }
    for child in view.subviews().iter() {
        emphasize(&child, on, rows);
    }
}
