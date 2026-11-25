//! Differential fuzzing against libghostty-vt (E11.3): generated output must leave the same text, cursor and core
//! attributes on the screen of `term-vt` as on Ghostty's; skipped where the library is not built.

use proptest::prelude::*;
use term_fuzz::ghostty::{self, Cell, Screen};
use term_fuzz::{config, ghostty_output};
use term_vt::cell::CellFlags;
use term_vt::color::Color;
use term_vt::style::Attrs;
use term_vt::{Size, Terminal};

const ATTRS: [(Attrs, &str); 6] = [
    (Attrs::BOLD, "bold"),
    (Attrs::ITALIC, "italic"),
    (Attrs::DIM, "dim"),
    (Attrs::INVERSE, "inverse"),
    (Attrs::HIDDEN, "hidden"),
    (Attrs::STRIKE, "strike"),
];

const UNDERLINES: [Attrs; 5] = [
    Attrs::UNDERLINE,
    Attrs::DOUBLE_UNDERLINE,
    Attrs::UNDERCURL,
    Attrs::DOTTED_UNDERLINE,
    Attrs::DASHED_UNDERLINE,
];

fn colour(color: Color) -> Option<String> {
    (color != Color::FOREGROUND && color != Color::BACKGROUND).then(|| color.reference())
}

fn own(bytes: &[u8], columns: u16, lines: u16) -> Screen {
    let mut terminal = Terminal::new(
        Size {
            columns: usize::from(columns),
            lines: usize::from(lines),
        },
        20,
    );
    terminal.advance(bytes);
    let rows = terminal
        .grid()
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    let style = terminal.style(cell.style);
                    let mut attrs = Vec::new();
                    attrs.extend(colour(style.fg).map(|fg| format!("fg={fg}")));
                    attrs.extend(colour(style.bg).map(|bg| format!("bg={bg}")));
                    attrs.extend(
                        ATTRS
                            .iter()
                            .filter(|(a, _)| style.attrs.contains(*a))
                            .map(|(_, n)| (*n).to_string()),
                    );
                    if let Some(kind) = UNDERLINES.iter().position(|u| style.attrs.contains(*u)) {
                        attrs.push(format!("underline={}", kind + 1));
                    }
                    if cell.flags.contains(CellFlags::WIDE) {
                        attrs.push("wide".into());
                    }

                    let text = if cell.flags.contains(CellFlags::WIDE_SPACER) {
                        String::new()
                    } else {
                        // The own core keeps a tab in the cell it skipped for copying; Ghostty leaves it blank.
                        let mut text = if cell.c == '\t' { ' ' } else { cell.c }.to_string();
                        if let Some(extra) = row.extra(column) {
                            text.push_str(&extra.zero_width);
                        }
                        text
                    };
                    Cell { text, attrs }
                })
                .collect()
        })
        .collect();
    let cursor = terminal.cursor();
    Screen {
        cursor: (cursor.line, cursor.column),
        rows,
    }
}

/// Where Ghostty is known to differ from xterm, Alacritty and the own core alike, so a case is not compared.
fn known_difference(output: &[u8]) -> Option<&'static str> {
    let has = |needle: &[u8]| output.windows(needle.len()).any(|window| window == needle);
    if has(b"\x1b[?7l") && has("\u{301}".as_bytes()) {
        return Some(
            "with autowrap off a combining mark at the right margin is lost instead of joining the last character",
        );
    }
    let background = term_fuzz::csi(output)
        .iter()
        .any(|(last, params)| *last == b'm' && params.iter().any(|p| (40..=47).contains(p) || *p == 48));
    if background && term_fuzz::may_split_wide(output) {
        return Some(
            "erasing a wide character that wrapped also paints the spacer it left on the line above, which xterm does not have",
        );
    }
    if has(b"\x1b[?1049h") && background {
        return Some(
            "entering the alternate screen clears it in the default background, where xterm uses the current one",
        );
    }
    if has("\u{301}".as_bytes()) && term_fuzz::csi(output).iter().any(|(last, _)| *last == b'b') {
        return Some("REP after a combining mark repeats the base character; xterm leaves which one open");
    }
    if term_fuzz::csi(output)
        .iter()
        .any(|(last, params)| matches!(last, b'J' | b'K') && params.first().is_some_and(|&p| p > 3))
    {
        return Some("an ED or EL of a mode that does not exist moves the cursor, where xterm ignores it");
    }
    if has(b"\x1bM") {
        return Some("RI at the top margin keeps a pending wrap, where xterm's CursorUp ends it");
    }
    if has(b"\x1b#8") {
        return Some("DECALN fills in the current colours, where xterm resets the rendition first");
    }
    None
}

/// `screen` with only the background compared on blank cells: the rest does not show on most of them, and the spacer
/// a wide character leaves where it wrapped, which xterm does not have, keeps the current style in the own core, as in
/// Alacritty, and only the background in Ghostty, also after an edit moved it.
fn visible(mut screen: Screen) -> Screen {
    for cell in screen.rows.iter_mut().flatten() {
        if cell.text == " " {
            cell.attrs.retain(|a| a.starts_with("bg="));
        }
    }
    screen
}

#[test]
fn ghostty_is_linked_or_says_why_not() {
    if !ghostty::AVAILABLE {
        eprintln!("libghostty-vt is not built; `make ghostty-vt` builds it. The comparison with Ghostty is skipped.");
        return;
    }
    let screen = ghostty::screen(b"\x1b[1mab\x1b[0m\r\n\xe6\xbc\xa2", 6, 2).expect("Ghostty makes a terminal");
    assert_eq!(screen.render(), own(b"\x1b[1mab\x1b[0m\r\n\xe6\xbc\xa2", 6, 2).render());
}

/// The shared settings, with room for the cases `known_difference` leaves out.
fn settings() -> proptest::test_runner::Config {
    let config = config();
    proptest::test_runner::Config {
        max_global_rejects: config.cases * 4,
        ..config
    }
}

proptest! {
    #![proptest_config(settings())]

    #[test]
    fn ghostty_and_the_own_core_leave_the_same_screen(output in ghostty_output(), columns in 4u16..20, lines in 2u16..8) {
        prop_assume!(known_difference(&output).is_none());
        if let Some(ghostty) = ghostty::screen(&output, columns, lines) {
            let (own, ghostty) = (visible(own(&output, columns, lines)).render(), visible(ghostty).render());
            let first = own.lines().zip(ghostty.lines()).find(|(a, b)| a != b);
            prop_assert!(own == ghostty, "own {:?} where Ghostty has {:?} after {:?}", first.map(|f| f.0), first.map(|f| f.1), String::from_utf8_lossy(&output));
        }
    }
}

/// `TERM_PROBE="<bytes with \e for ESC>" cargo test -p term-fuzz --test ghostty probe -- --ignored --nocapture`
/// prints both screens of one output, for narrowing a difference down.
#[test]
#[ignore = "a tool, run by hand"]
fn probe() {
    let input = std::env::var("TERM_PROBE").unwrap_or_default().replace("\\e", "\x1b");
    let (columns, lines) = (
        std::env::var("TERM_PROBE_COLUMNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
        std::env::var("TERM_PROBE_LINES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3),
    );
    println!("own:\n{}", own(input.as_bytes(), columns, lines).render());
    if let Some(ghostty) = ghostty::screen(input.as_bytes(), columns, lines) {
        println!("ghostty:\n{}", ghostty.render());
    }
}
