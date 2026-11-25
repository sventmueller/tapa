#![allow(clippy::unwrap_used, reason = "tests")]

use super::*;

fn mode(rows: &[&str], at: (usize, usize)) -> CopyMode {
    let text = RowsText {
        rows: rows.iter().map(ToString::to_string).collect(),
        cells: HashMap::new(),
        first: 1000,
        cursor: (0, 0),
        ..RowsText::default()
    };
    CopyMode::new(text, at)
}

fn keys(copy: &mut CopyMode, keys: &str) -> Step {
    let mut last = Step::Ignored;
    for c in keys.chars() {
        last = copy.key(CopyKey::Char(c), 10);
    }
    last
}

#[test]
fn vims_motions_move_the_cursor_by_rows_and_cells() {
    let mut copy = mode(&["one two three", "four", "", "five six"], (0, 0));
    keys(&mut copy, "w");
    assert_eq!(copy.cursor(), (1000, 4));
    keys(&mut copy, "e");
    assert_eq!(copy.cursor(), (1000, 6));
    keys(&mut copy, "$");
    assert_eq!(copy.cursor(), (1000, 12));
    keys(&mut copy, "0j");
    assert_eq!(copy.cursor(), (1001, 0));
    keys(&mut copy, "G");
    assert_eq!(copy.cursor().0, 1003);
    keys(&mut copy, "gg");
    assert_eq!(copy.cursor(), (1000, 0));
    keys(&mut copy, "b");
    assert_eq!(copy.cursor(), (1000, 0));
    keys(&mut copy, "fh");
    assert_eq!(copy.cursor(), (1000, 9));
    keys(&mut copy, "2l");
    assert_eq!(copy.cursor(), (1000, 11));
}

#[test]
fn half_and_whole_pages_jump_by_the_views_rows_within_the_text() {
    let rows: Vec<String> = (0..50).map(|n| format!("row{n}")).collect();
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let mut copy = mode(&rows, (49, 0));
    assert_eq!(copy.key(CopyKey::Ctrl('u'), 10), Step::Moved);
    assert_eq!(copy.cursor().0, 1044);
    copy.key(CopyKey::Ctrl('b'), 10);
    assert_eq!(copy.cursor().0, 1034);
    copy.key(CopyKey::Ctrl('d'), 10);
    assert_eq!(copy.cursor().0, 1039);
    copy.key(CopyKey::PageDown, 100);
    assert_eq!(copy.cursor().0, 1049);
    copy.key(CopyKey::Up, 10);
    assert_eq!(copy.cursor().0, 1048);
}

#[test]
fn v_selects_characters_capital_v_rows_and_ctrl_v_a_block() {
    let mut copy = mode(&["alpha beta", "gamma delta", "epsilon"], (0, 2));
    assert_eq!(copy.selection(), None);
    keys(&mut copy, "vj");
    assert_eq!(copy.selection(), Some((SelectionKind::Simple, (1000, 2), (1001, 2))));
    copy.key(CopyKey::Ctrl('d'), 2);
    assert_eq!(copy.selection(), Some((SelectionKind::Simple, (1000, 2), (1002, 2))));
    keys(&mut copy, "V");
    assert_eq!(copy.selection(), Some((SelectionKind::Line, (1000, 0), (1002, 0))));
    copy.key(CopyKey::Escape, 10);
    assert_eq!(copy.selection(), None);
    keys(&mut copy, "ggw");
    copy.key(CopyKey::Ctrl('v'), 10);
    keys(&mut copy, "jll");
    assert_eq!(copy.selection(), Some((SelectionKind::Block, (1000, 6), (1001, 8))));
    keys(&mut copy, "hhhh");
    assert_eq!(copy.selection(), Some((SelectionKind::Block, (1000, 4), (1001, 6))));
}

#[test]
fn y_copies_q_and_escape_leave_and_keys_that_would_edit_do_nothing() {
    let mut copy = mode(&["text"], (0, 0));
    for key in "xdipaAoOrsuJ~p.".chars() {
        assert_eq!(copy.key(CopyKey::Char(key), 10), Step::Ignored, "{key}");
    }
    assert_eq!(keys(&mut copy, "y"), Step::Copy);
    assert_eq!(copy.key(CopyKey::Enter, 10), Step::Copy);
    assert_eq!(keys(&mut copy, "q"), Step::Leave);
    assert_eq!(copy.key(CopyKey::Escape, 10), Step::Leave);
    keys(&mut copy, "v");
    assert_eq!(copy.key(CopyKey::Escape, 10), Step::Moved);
    assert_eq!(copy.status().mode, Mode::Normal);
}

#[test]
fn slash_and_question_mark_search_forward_and_back_and_n_repeats() {
    let mut copy = mode(&["a match", "none", "match again", "end"], (3, 0));
    keys(&mut copy, "?match");
    assert_eq!(copy.status().prompt.as_deref(), Some("?match"));
    // `q` and `y` are part of what is typed while the prompt is open.
    assert_eq!(copy.key(CopyKey::Enter, 10), Step::Moved);
    assert_eq!(copy.cursor(), (1002, 0));
    keys(&mut copy, "n");
    assert_eq!(copy.cursor(), (1000, 2));
    keys(&mut copy, "/again");
    copy.key(CopyKey::Enter, 10);
    assert_eq!(copy.cursor(), (1002, 6));
    keys(&mut copy, "/qy");
    copy.key(CopyKey::Enter, 10);
    assert_eq!(copy.status().note, Some(Note::PatternNotFound("qy".into())));
    assert_eq!(copy.cursor(), (1002, 6));
}

#[test]
fn wide_characters_map_to_their_cells_both_ways() {
    let mut cells = HashMap::new();
    cells.insert(0, vec![0, 2, 4, 5]);
    let text = RowsText {
        rows: vec!["日本 x".into()],
        cells,
        first: 0,
        cursor: (0, 0),
        ..RowsText::default()
    };
    let mut copy = CopyMode::new(text, (0, 3));
    assert_eq!(copy.cursor(), (0, 2));
    assert_eq!(copy.cursor_width(), 2);
    keys(&mut copy, "l");
    assert_eq!((copy.cursor(), copy.cursor_width()), ((0, 4), 1));
    keys(&mut copy, "$");
    assert_eq!(copy.cursor(), (0, 5));
    let zero_width = HashMap::from([(0, vec![0, 1, 1, 2])]);
    assert_eq!(char_of(&zero_width, 0, 1), 1);
    assert_eq!(cell_of(&zero_width, 0, 6), 5);
}

#[test]
fn keys_read_as_vim_writes_them() {
    assert_eq!(
        parse("v<C-v>j<Esc><CR><x"),
        [
            CopyKey::Char('v'),
            CopyKey::Ctrl('v'),
            CopyKey::Char('j'),
            CopyKey::Escape,
            CopyKey::Enter,
            CopyKey::Char('<'),
            CopyKey::Char('x'),
        ]
    );
}

fn rows_text(rows: &[&str], first: u64, history: usize) -> RowsText {
    RowsText {
        rows: rows.iter().map(ToString::to_string).collect(),
        first,
        history,
        ..RowsText::default()
    }
}

#[test]
fn new_output_reaches_copy_mode_and_the_cursor_stays_on_its_row() {
    let mut copy = CopyMode::new(rows_text(&["a", "b", "c", "d"], 10, 2), (1, 0));
    keys(&mut copy, "vj");
    copy.refresh(rows_text(&["b", "c", "d", "e", "f"], 11, 3));
    assert_eq!(copy.cursor(), (12, 0));
    assert_eq!(
        copy.selection().map(|(_, from, to)| (from, to)),
        Some(((11, 0), (12, 0)))
    );
    keys(&mut copy, "G");
    assert_eq!(copy.cursor(), (15, 0));
}
