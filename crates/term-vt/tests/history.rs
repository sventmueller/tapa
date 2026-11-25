//! A packed history (F8) reads, searches, reflows and renumbers its styles as the same history left open.

use term_vt::reference::dump;
use term_vt::search::{Options, compile, find_all};
use term_vt::{Size, Terminal};

const HISTORY: usize = 10_000;

fn output(lines: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for n in 0..lines {
        let colour = 31 + n % 7;
        let pad = "x".repeat(n % 90);
        out.extend_from_slice(
            format!(
                "\x1b]133;A\x07$ cmd {n}\r\n\x1b[1;{colour}mbold {n}\x1b[0m 中文 e\u{301} \
                 \x1b]8;;http://x/{n}\x07link\x1b]8;;\x07 {pad}\r\n"
            )
            .as_bytes(),
        );
    }
    out
}

fn pair(bytes: &[u8]) -> (Terminal, Terminal) {
    let size = Size { columns: 80, lines: 24 };
    let mut open = Terminal::new(size, HISTORY);
    let mut packed = Terminal::new(size, HISTORY);
    open.advance(bytes);
    packed.advance(bytes);
    assert!(packed.grid_mut().history.pack(0, usize::MAX) > 1000);
    (open, packed)
}

fn same(open: &Terminal, packed: &Terminal, when: &str) {
    assert_eq!(dump(open, HISTORY), dump(packed, HISTORY), "{when}");
    assert_eq!(open.prompt_lines(), packed.prompt_lines(), "{when}");
    let expression = compile(
        "bold 1[0-9]+|中文",
        Options {
            regex: true,
            ..Options::default()
        },
    )
    .expect("the expression is valid");
    assert_eq!(
        find_all(open.grid(), &expression, usize::MAX),
        find_all(packed.grid(), &expression, usize::MAX),
        "{when}"
    );
}

#[test]
fn a_packed_history_is_the_same_history() {
    let (mut open, mut packed) = pair(&output(3000));
    same(&open, &packed, "packed");

    for size in [
        Size { columns: 50, lines: 30 },
        Size {
            columns: 120,
            lines: 10,
        },
    ] {
        open.resize(size);
        packed.resize(size);
        same(&open, &packed, "after a resize");
    }

    packed.grid_mut().history.pack(0, usize::MAX);
    let more = output(500);
    open.advance(&more);
    packed.advance(&more);
    same(&open, &packed, "after more output");
}

#[test]
fn the_style_table_is_compacted_over_packed_rows() {
    let (mut open, mut packed) = pair(&output(2000));
    let mut colours = Vec::new();
    for n in 0..70_000u32 {
        let [_, r, g, b] = n.to_be_bytes();
        colours.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m#").as_bytes());
    }
    open.advance(&colours);
    packed.advance(&colours);
    same(&open, &packed, "after the style table filled");
}

#[test]
fn idle_packing_works_in_steps_keeps_the_view_and_saves_memory() {
    let mut terminal = Terminal::new(
        Size {
            columns: 200,
            lines: 24,
        },
        HISTORY,
    );
    terminal.advance(&output(3000));
    terminal.grid_mut().scroll_view(1000);
    let view = |t: &Terminal| -> Vec<String> { (0..24).map(|line| t.grid().view_row(line).unwrap().text()).collect() };
    let shown = view(&terminal);
    let open = terminal.grid().history.bytes();
    let mut steps = 0;
    while terminal.pack_history(1000) > 0 {
        steps += 1;
    }
    assert!(
        steps >= 2,
        "{steps} steps for {} rows, a page each",
        terminal.grid().history.len()
    );
    assert_eq!(view(&terminal), shown);
    let packed = terminal.grid().history.bytes();
    let full = terminal.grid().history.len() * 200 * 8;
    println!("history of 200 columns: {full} bytes at full width, {open} kept, {packed} packed");
    assert!(open * 3 < full, "pages keep only written cells: {open} of {full}");
    assert!(packed * 3 < open * 2, "packing saves a third more: {packed} of {open}");
}
