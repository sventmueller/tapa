use super::*;
use crate::syntax::Language;

/// Marks a caret in the texts of the tables.
const CARET: char = '│';
/// Marks where a selection starts.
const FROM: char = '⟨';
/// Marks where a selection ends.
const TO: char = '⟩';

/// The text without its marks, and its selections in UTF-16 units in the order they appear.
fn parse(marked: &str) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut selections = vec![];
    let mut from = None;
    for c in marked.chars() {
        let at = utf16_len(&text);
        match c {
            CARET => selections.push(at..at),
            FROM => from = Some(at),
            TO => selections.push(from.take().expect("a selection that started")..at),
            _ => text.push(c),
        }
    }
    (text, selections)
}

/// `text` with `selections` marked as `parse` reads them.
fn mark(text: &str, selections: &[Range<usize>]) -> String {
    let marks = |at: usize, out: &mut String| {
        for selection in selections {
            if !selection.is_empty() && selection.end == at {
                out.push(TO);
            }
        }
        for selection in selections {
            if selection.is_empty() && selection.start == at {
                out.push(CARET);
            } else if !selection.is_empty() && selection.start == at {
                out.push(FROM);
            }
        }
    };
    let mut out = String::new();
    let mut at = 0;
    for c in text.chars() {
        marks(at, &mut out);
        out.push(c);
        at += c.len_utf16();
    }
    marks(at, &mut out);
    out
}

fn replaced(text: &str, range: Range<usize>, with: &str) -> String {
    let (start, end) = (byte_of(text, range.start), byte_of(text, range.end));
    format!("{}{with}{}", &text[..start], &text[end..])
}

/// The text and the selection after the view makes `step` over `selection`.
fn apply(text: &str, selection: Range<usize>, step: Step) -> (String, Range<usize>) {
    match step {
        Step::Move(at) => (text.to_string(), at..at),
        Step::Type { text: typed, select } => {
            let end = selection.start + utf16_len(&typed);
            (replaced(text, selection, &typed), select.unwrap_or(end..end))
        }
        Step::Replace { range, text: new } => {
            let start = range.start;
            (replaced(text, range, &new), start..start + utf16_len(&new))
        }
    }
}

/// Makes the step `at` gives at every selection of `marked`, from the last to the first as the editor's carets do,
/// moving the selections done by how much the text grew; the result marked again.
fn at_each(marked: &str, at: impl Fn(&str, Range<usize>) -> Step) -> String {
    let (mut text, selections) = parse(marked);
    let mut done: Vec<Range<usize>> = vec![];
    for selection in selections.into_iter().rev() {
        let before = utf16_len(&text);
        let step = at(&text, selection.clone());
        let (changed, now) = apply(&text, selection, step);
        let after = utf16_len(&changed);
        let moved = |at: usize| (at + after).saturating_sub(before);
        for range in &mut done {
            *range = moved(range.start)..moved(range.end);
        }
        done.insert(0, now);
        text = changed;
    }
    mark(&text, &done)
}

/// What typing `c` does, as the view makes it: the step, or else the character as typed.
fn typing(c: char) -> impl Fn(&str, Range<usize>) -> Step {
    move |text, selection| {
        typed(text, selection, c).unwrap_or(Step::Type {
            text: c.to_string(),
            select: None,
        })
    }
}

#[test]
fn the_marks_of_the_tables_read_back() {
    for marked in ["", "│", "a│b", "⟨ab⟩c", "⟨a⟩│b", "😀│ä", "a│\r\n│b"] {
        let (text, selections) = parse(marked);
        assert_eq!(mark(&text, &selections), marked);
    }
}

#[test]
fn brackets_and_quotes_pair_wrap_and_step_over() {
    let table: &[(&str, char, &str, &str)] = &[
        ("│", '(', "(│)", "a bracket in an empty text gets its pair"),
        ("x│", '(', "x(│)", "a bracket at the end of a line gets its pair"),
        (
            "│ x",
            '[',
            "[│] x",
            "white space after the caret leaves room for a pair",
        ),
        (
            "│)",
            '(',
            "(│))",
            "a closing bracket after the caret leaves room for a pair",
        ),
        (
            "a│;",
            '{',
            "a{│};",
            "a semicolon after the caret leaves room for a pair",
        ),
        (
            "│\n",
            '{',
            "{│}\n",
            "a line break after the caret leaves room for a pair",
        ),
        ("│\r\n", '`', "`│`\r\n", "a CRLF after the caret leaves room for a pair"),
        ("│x", '(', "(│x", "a bracket in front of a word stays single"),
        (
            "f(│)",
            ')',
            "f()│",
            "a closing bracket in front of itself is stepped over",
        ),
        ("\"│\"", '"', "\"\"│", "a quote in front of itself is stepped over"),
        ("f(│)", ']', "f(]│)", "another closing bracket goes in"),
        ("│ ", '}', "}│ ", "a closing bracket with none after it goes in"),
        ("│", '"', "\"│\"", "a quote gets its pair"),
        ("x = │", '\'', "x = '│'", "a quote after a space gets its pair"),
        ("it│", '\'', "it'│", "a quote right after a letter stays single"),
        ("\"│", '"', "\"\"│", "a quote right after a quote stays single"),
        (
            "ä│",
            '"',
            "ä\"│",
            "a quote right after a letter beyond ASCII stays single",
        ),
        ("😀│", '(', "😀(│)", "a pair after a character of two UTF-16 units"),
        ("│😀", '(', "(│😀", "a bracket in front of an emoji stays single"),
        (
            "⟨x⟩",
            '[',
            "[⟨x⟩]",
            "a bracket typed over a selection wraps it and keeps it selected",
        ),
        ("⟨x⟩", '"', "\"⟨x⟩\"", "a quote typed over a selection wraps it"),
        (
            "⟨ä😀⟩",
            '(',
            "(⟨ä😀⟩)",
            "a wrapped selection beyond ASCII stays selected",
        ),
        ("│x", 'a', "a│x", "a letter goes in as typed"),
    ];
    for (before, c, after, what) in table {
        assert_eq!(at_each(before, typing(*c)), *after, "{what}");
    }
}

#[test]
fn letters_are_left_to_the_view() {
    assert_eq!(typed("ab", 1..1, 'x'), None);
    assert_eq!(typed("ab", 0..2, '-'), None);
}

#[test]
fn every_caret_pairs_and_steps_over_on_its_own() {
    let table: &[(&str, char, &str)] = &[
        ("a│ b│", '(', "a(│) b(│)"),
        ("f(│) g(│)", ')', "f()│ g()│"),
        ("│x │", '(', "(│x (│)"),
        ("⟨a⟩ ⟨b⟩", '"', "\"⟨a⟩\" \"⟨b⟩\""),
        ("x│\ny│", '\'', "x'│\ny'│"),
    ];
    for (before, c, after) in table {
        assert_eq!(at_each(before, typing(*c)), *after, "{before:?} typing {c:?}");
    }
}

#[test]
fn return_indents_like_the_line_above_and_deeper_after_an_opening() {
    let table: &[(&str, &str, &str, &str, &str)] = &[
        (
            "fn f() {│}",
            "\n",
            "    ",
            "fn f() {\n    │\n}",
            "between braces the closing one moves down",
        ),
        (
            "  foo(│)",
            "\n",
            "    ",
            "  foo(\n      │\n  )",
            "between brackets the closing one keeps the indentation",
        ),
        ("[│]", "\r", "  ", "[\r  │\r]", "a lone CR as the line break"),
        ("a {│}", "\r\n", "    ", "a {\r\n    │\r\n}", "CRLF as the line break"),
        (
            "def f():│",
            "\n",
            "    ",
            "def f():\n    │",
            "after a colon one level deeper",
        ),
        ("\tif x {│", "\n", "\t", "\tif x {\n\t\t│", "a tab as the unit"),
        (
            "if x {  │",
            "\n",
            "    ",
            "if x {  \n    │",
            "white space after the opening one",
        ),
        ("f(│x", "\n", "    ", "f(\n    │x", "an opening without its closing one"),
        (
            "    x = 1│",
            "\n",
            "    ",
            "    x = 1\n    │",
            "the indentation of the line above",
        ),
        ("x\n  y│", "\n", "    ", "x\n  y\n  │", "the caret's own line counts"),
        (
            "a\r\n\tb│",
            "\r\n",
            "\t",
            "a\r\n\tb\r\n\t│",
            "the caret's own line after a CRLF",
        ),
        ("x│}", "\n", "    ", "x\n│}", "a closing bracket alone does not indent"),
        ("  a⟨bc⟩d", "\n", "    ", "  a\n  │d", "the selection goes"),
        ("ä {│}", "\n", "  ", "ä {\n  │\n}", "a line beyond ASCII"),
        (
            "\u{3000}x:│",
            "\n",
            "    ",
            "\u{3000}x:\n    │",
            "only spaces and tabs count as indentation",
        ),
    ];
    for (before, ending, unit, after, what) in table {
        let step = |text: &str, selection| newline(text, selection, ending, unit);
        assert_eq!(at_each(before, step), *after, "{what}");
    }
}

#[test]
fn return_at_every_caret_splits_each_pair() {
    let step = |text: &str, selection| newline(text, selection, "\n", "    ");
    assert_eq!(at_each("a {│}\nb {│}", step), "a {\n    │\n}\nb {\n    │\n}");
    assert_eq!(at_each("  x│\ny│", step), "  x\n  │\ny\n│");
}

#[test]
fn tab_inserts_a_unit_or_indents_the_selected_lines() {
    let table: &[(&str, &str, &str, &str)] = &[
        ("│x", "    ", "    │x", "a caret gets a unit"),
        ("a⟨b⟩c", "\t", "a\t│c", "a selection within a line becomes a unit"),
        ("⟨a\nb⟩", "  ", "⟨  a\n  b⟩", "each selected line"),
        (
            "x⟨a\nb⟩y\nz",
            "  ",
            "⟨  xa\n  by\n⟩z",
            "whole lines, the one after them untouched",
        ),
        (
            "⟨a\n⟩b",
            "  ",
            "⟨  a\n⟩b",
            "not the line the selection only reaches the start of",
        ),
        ("⟨a\n\nb\n⟩", "  ", "⟨  a\n\n  b\n⟩", "not the empty line"),
        ("⟨a\n   \nb⟩", "\t", "⟨\ta\n   \n\tb⟩", "not the line of white space"),
        ("⟨a\r\nb⟩\r\n", "  ", "⟨  a\r\n  b\r\n⟩", "CRLF lines"),
        ("⟨a\rb⟩", "  ", "⟨  a\r  b⟩", "lone CR lines"),
        ("⟨ä\n😀⟩", "  ", "⟨  ä\n  😀⟩", "lines beyond ASCII"),
    ];
    for (before, unit, after, what) in table {
        let step = |text: &str, selection| tab(text, selection, unit);
        assert_eq!(at_each(before, step), *after, "{what}");
    }
}

#[test]
fn tab_at_every_caret_indents_each() {
    let step = |text: &str, selection| tab(text, selection, "  ");
    assert_eq!(at_each("│a\n│b", step), "  │a\n  │b");
}

#[test]
fn comments_go_out_and_in_again() {
    let table: &[(&str, &str, &str, &str)] = &[
        ("│let x;", "//", "⟨// let x;⟩", "a line with the caret"),
        ("⟨// let x;⟩", "//", "⟨let x;⟩", "a comment comes in again"),
        (
            "⟨//let x;⟩",
            "//",
            "⟨let x;⟩",
            "a comment without a space comes in again",
        ),
        ("a\nb│c\nd", "#", "a\n⟨# bc\n⟩d", "only the caret's line"),
        ("⟨    a\n  b\n⟩", "#", "⟨  #   a\n  # b\n⟩", "at the least indentation"),
        (
            "⟨  # a\n    # b⟩",
            "#",
            "⟨  a\n    b⟩",
            "the indentation stays when commenting in",
        ),
        (
            "⟨// a\nb⟩",
            "//",
            "⟨// // a\n// b⟩",
            "a mixed selection is commented out",
        ),
        ("⟨a\n\n  b⟩", "--", "⟨-- a\n\n--   b⟩", "not the empty line"),
        (
            "⟨-- a\n  \n-- b⟩",
            "--",
            "⟨a\n  \nb⟩",
            "a line of white space does not count",
        ),
        ("⟨\n\n⟩", "//", "⟨\n\n⟩", "only empty lines stay as they are"),
        ("⟨a\r\nb⟩\r\n", "//", "⟨// a\r\n// b\r\n⟩", "CRLF lines"),
        ("⟨// a\r\n// b\r\n⟩", "//", "⟨a\r\nb\r\n⟩", "CRLF lines commented in"),
        ("⟨ä😀⟩", "//", "⟨// ä😀⟩", "a line beyond ASCII"),
        (
            "⟨\u{3000}a\n\u{3000}\u{3000}b⟩",
            "#",
            "⟨\u{3000}# a\n\u{3000}# \u{3000}b⟩",
            "wide spaces are counted as characters",
        ),
    ];
    for (before, marker, after, what) in table {
        let step = |text: &str, selection| toggle_comment(text, selection, marker);
        assert_eq!(at_each(before, step), *after, "{what}");
    }
}

#[test]
fn the_comment_marker_follows_the_language() {
    let marker = |name: &str| Language::named(name).expect("a language splat knows").line_comment();
    assert_eq!(marker("python"), Some("#"));
    assert_eq!(marker("bash"), Some("#"));
    assert_eq!(marker("sql"), Some("--"));
    assert_eq!(marker("rust"), Some("//"));
    assert_eq!(marker("css"), None);
    assert_eq!(marker("html"), None);
    assert_eq!(Language::MARKDOWN.line_comment(), None);
}

#[test]
fn reindenting_follows_the_line_above_and_the_brackets_as_return_does() {
    let whole = |text: &str, unit: &str| {
        let Step::Replace { range, text: indented } = reindent(text, 0..utf16_len(text), unit) else {
            panic!("a replacement");
        };
        assert_eq!(range, 0..utf16_len(text));
        indented
    };
    assert_eq!(whole("a {\nb\n    }\n", "    "), "a {\n    b\n}\n");
    assert_eq!(whole("if x:\n  y\nz\n", "\t"), "if x:\n\ty\n\tz\n");
    assert_eq!(whole("f(\r\n  a,\r\n   \r\n)", "  "), "f(\r\n  a,\r\n\r\n)");
    assert_eq!(whole("\t\ta {\n}\n", "    "), "a {\n}\n");
    let second = reindent("  a\nb\n", 4..5, "    ");
    assert_eq!(
        second,
        Step::Replace {
            range: 4..6,
            text: "  b\n".into()
        },
        "the line above the selection counts as it is"
    );
    let below_blank = reindent("  a {\n\n  b\n", 7..7, "  ");
    assert_eq!(
        below_blank,
        Step::Replace {
            range: 7..11,
            text: "    b\n".into()
        },
        "a blank line above counts as none"
    );
    assert_eq!(one_level_less("\t\t", "    "), "\t");
    assert_eq!(one_level_less("      ", "    "), "  ");
    assert_eq!(one_level_less("  ", "    "), "");
}

#[test]
fn a_piece_of_the_text_gives_the_same_step_as_the_whole() {
    let whole = "fn f() {\n    g(x)\n}\n";
    let piece = &whole[9..18];
    assert_eq!(
        typed(piece, 7..7, ')').map(|step| shifted(step, 9)),
        typed(whole, 16..16, ')')
    );
    assert_eq!(
        shifted(newline(piece, 6..6, "\n", "    "), 9),
        newline(whole, 15..15, "\n", "    ")
    );
    assert_eq!(shifted(tab(piece, 0..9, "  "), 9), tab(whole, 9..18, "  "));
}

fn shifted(step: Step, by: usize) -> Step {
    let range = |r: Range<usize>| r.start + by..r.end + by;
    match step {
        Step::Move(at) => Step::Move(at + by),
        Step::Type { text, select } => Step::Type {
            text,
            select: select.map(range),
        },
        Step::Replace { range: r, text } => Step::Replace { range: range(r), text },
    }
}
