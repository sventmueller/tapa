use std::time::{Duration, Instant};

use super::*;

/// One case: the text before, the cursor before, the keys, the text after and the cursor after.
struct Case {
    text: &'static str,
    at: (usize, usize),
    keys: &'static str,
    after: &'static str,
    cursor: (usize, usize),
}

const CASES: &[Case] = &[
    // Motions
    Case {
        text: "one two three\n",
        at: (0, 0),
        keys: "w",
        after: "one two three\n",
        cursor: (0, 4),
    },
    Case {
        text: "one two three\n",
        at: (0, 8),
        keys: "b",
        after: "one two three\n",
        cursor: (0, 4),
    },
    Case {
        text: "one two three\n",
        at: (0, 0),
        keys: "2w",
        after: "one two three\n",
        cursor: (0, 8),
    },
    Case {
        text: "one two three\n",
        at: (0, 0),
        keys: "e",
        after: "one two three\n",
        cursor: (0, 2),
    },
    Case {
        text: "one two three\n",
        at: (0, 4),
        keys: "$",
        after: "one two three\n",
        cursor: (0, 12),
    },
    Case {
        text: "  indented\n",
        at: (0, 8),
        keys: "^",
        after: "  indented\n",
        cursor: (0, 2),
    },
    Case {
        text: "  indented\n",
        at: (0, 8),
        keys: "0",
        after: "  indented\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "j",
        after: "a\nb\nc\n",
        cursor: (1, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (2, 0),
        keys: "gg",
        after: "a\nb\nc\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "G",
        after: "a\nb\nc\n",
        cursor: (2, 0),
    },
    Case {
        text: "f(a, b)\n",
        at: (0, 0),
        keys: "f,",
        after: "f(a, b)\n",
        cursor: (0, 3),
    },
    Case {
        text: "f(a, b)\n",
        at: (0, 1),
        keys: "%",
        after: "f(a, b)\n",
        cursor: (0, 6),
    },
    // Operators with motions
    Case {
        text: "one\ntwo\nthree\n",
        at: (1, 0),
        keys: "dd",
        after: "one\nthree\n",
        cursor: (1, 0),
    },
    Case {
        text: "one two three\n",
        at: (0, 0),
        keys: "dw",
        after: "two three\n",
        cursor: (0, 0),
    },
    Case {
        text: "one two three\n",
        at: (0, 4),
        keys: "d$",
        after: "one \n",
        cursor: (0, 3),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "x",
        after: "ne two\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\nc\nd\n",
        at: (0, 0),
        keys: "2dd",
        after: "c\nd\n",
        cursor: (0, 0),
    },
    Case {
        text: "one\ntwo\n",
        at: (0, 0),
        keys: "yyp",
        after: "one\none\ntwo\n",
        cursor: (1, 0),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "cwnew<Esc>",
        after: "new two\n",
        cursor: (0, 2),
    },
    Case {
        text: "abc\n",
        at: (0, 0),
        keys: "~",
        after: "Abc\n",
        cursor: (0, 1),
    },
    Case {
        text: "a b\n",
        at: (0, 0),
        keys: "rX",
        after: "X b\n",
        cursor: (0, 0),
    },
    // Text objects
    Case {
        text: "say hello world\n",
        at: (0, 6),
        keys: "ciwbye<Esc>",
        after: "say bye world\n",
        cursor: (0, 6),
    },
    Case {
        text: "f(a, b)\n",
        at: (0, 3),
        keys: "di(",
        after: "f()\n",
        cursor: (0, 2),
    },
    Case {
        text: "x = \"str\";\n",
        at: (0, 6),
        keys: "ci\"s<Esc>",
        after: "x = \"s\";\n",
        cursor: (0, 5),
    },
    Case {
        text: "one two three\n",
        at: (0, 5),
        keys: "daw",
        after: "one three\n",
        cursor: (0, 4),
    },
    // Repeat
    Case {
        text: "a b c\n",
        at: (0, 0),
        keys: "ciwX<Esc>w.",
        after: "X X c\n",
        cursor: (0, 2),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "dd.",
        after: "c\n",
        cursor: (0, 0),
    },
    // Insert forms
    Case {
        text: "abc\n",
        at: (0, 1),
        keys: "aX<Esc>",
        after: "abXc\n",
        cursor: (0, 2),
    },
    Case {
        text: "abc\n",
        at: (0, 1),
        keys: "AX<Esc>",
        after: "abcX\n",
        cursor: (0, 3),
    },
    Case {
        text: "  abc\n",
        at: (0, 4),
        keys: "IX<Esc>",
        after: "  Xabc\n",
        cursor: (0, 2),
    },
    Case {
        text: "a\nb\n",
        at: (0, 0),
        keys: "oX<Esc>",
        after: "a\nX\nb\n",
        cursor: (1, 0),
    },
    Case {
        text: "a\nb\n",
        at: (1, 0),
        keys: "OX<Esc>",
        after: "a\nX\nb\n",
        cursor: (1, 0),
    },
    // Visual
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "vlld",
        after: " two\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "Vjd",
        after: "c\n",
        cursor: (0, 0),
    },
    // Registers and macros
    Case {
        text: "one\ntwo\n",
        at: (0, 0),
        keys: "\"ayyj\"ap",
        after: "one\ntwo\none\n",
        cursor: (2, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "qqA;<Esc>jq@q",
        after: "a;\nb;\nc\n",
        cursor: (2, 0),
    },
    // Search and Ex
    Case {
        text: "alpha\nbeta\ngamma beta\n",
        at: (0, 0),
        keys: "/beta<Enter>",
        after: "alpha\nbeta\ngamma beta\n",
        cursor: (1, 0),
    },
    Case {
        text: "alpha\nbeta\ngamma beta\n",
        at: (0, 0),
        keys: "/beta<Enter>n",
        after: "alpha\nbeta\ngamma beta\n",
        cursor: (2, 6),
    },
    Case {
        text: "alpha\nbeta\ngamma beta\n",
        at: (2, 8),
        keys: "?beta<Enter>",
        after: "alpha\nbeta\ngamma beta\n",
        cursor: (2, 6),
    },
    Case {
        text: "x x\ny x\n",
        at: (0, 0),
        keys: ":%s/x/z/g<Enter>",
        after: "z z\ny z\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: ":3<Enter>",
        after: "a\nb\nc\n",
        cursor: (2, 0),
    },
    Case {
        text: "one\ntwo\n",
        at: (0, 0),
        keys: "/two<Enter>",
        after: "one\ntwo\n",
        cursor: (1, 0),
    },
    Case {
        text: "x x\ny x\n",
        at: (0, 0),
        keys: ":s#x#z#<Enter>",
        after: "z x\ny x\n",
        cursor: (0, 0),
    },
    Case {
        text: "x x\ny x\n",
        at: (0, 0),
        keys: ":%s#x#z#g<Enter>",
        after: "z z\ny z\n",
        cursor: (0, 0),
    },
    Case {
        text: "x x\ny x\n",
        at: (0, 0),
        keys: ":s#x#z#<Enter>x",
        after: " x\ny x\n",
        cursor: (0, 0),
    },
    // A key that waits for a character or a register takes the next one, search keys too
    Case {
        text: "axbxcxd\n",
        at: (0, 0),
        keys: "fx;x",
        after: "axbcxd\n",
        cursor: (0, 3),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "fn",
        after: "one two\n",
        cursor: (0, 1),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "rn",
        after: "nne two\n",
        cursor: (0, 0),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "dnx",
        after: "ne two\n",
        cursor: (0, 0),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "d/x",
        after: "ne two\n",
        cursor: (0, 0),
    },
    Case {
        text: "one\ntwo\nthree\n",
        at: (0, 0),
        keys: "\"*yyj\"*p",
        after: "one\ntwo\none\nthree\n",
        cursor: (2, 0),
    },
    Case {
        text: "alpha beta\nbeta\n",
        at: (0, 0),
        keys: "w*",
        after: "alpha beta\nbeta\n",
        cursor: (1, 0),
    },
    Case {
        text: "alpha beta\nbeta\n",
        at: (1, 0),
        keys: "#",
        after: "alpha beta\nbeta\n",
        cursor: (0, 6),
    },
    // Lines at the end of the text
    Case {
        text: "a\nb\nc\n",
        at: (0, 0),
        keys: "jdG",
        after: "a\n",
        cursor: (0, 0),
    },
    Case {
        text: "one\ntwo\nthree\n",
        at: (0, 0),
        keys: "yyjddG\"0p",
        after: "one\nthree\none\n",
        cursor: (2, 0),
    },
    Case {
        text: "one\ntwo\nthree\n",
        at: (0, 0),
        keys: "jddG\"1p",
        after: "one\nthree\ntwo\n",
        cursor: (2, 0),
    },
    Case {
        text: "one\ntwo\nthree\n",
        at: (0, 0),
        keys: "yyj\"_ddGp",
        after: "one\nthree\none\n",
        cursor: (2, 0),
    },
    Case {
        text: "one\ntwo\nthree\n",
        at: (0, 0),
        keys: "\"ayyj\"AyyG\"ap",
        after: "one\ntwo\nthree\none\ntwo\n",
        cursor: (3, 0),
    },
    Case {
        text: "a\nb\n",
        at: (0, 0),
        keys: "Gdd",
        after: "a\n",
        cursor: (0, 0),
    },
    Case {
        text: "a\nb\n",
        at: (0, 0),
        keys: "GoX<Esc>",
        after: "a\nb\nX\n",
        cursor: (2, 0),
    },
    // Visual block, gv and U
    Case {
        text: "abc\ndef\nghi\n",
        at: (0, 0),
        keys: "<C-V>jIX<Esc>",
        after: "Xabc\nXdef\nghi\n",
        cursor: (0, 0),
    },
    Case {
        text: "abc\ndef\nghi\n",
        at: (0, 0),
        keys: "<C-V>jAX<Esc>",
        after: "aXbc\ndXef\nghi\n",
        cursor: (0, 1),
    },
    Case {
        text: "abc def\n",
        at: (0, 0),
        keys: "vl<Esc>gvd",
        after: "c def\n",
        cursor: (0, 0),
    },
    Case {
        text: "abc def\n",
        at: (0, 0),
        keys: "vl<Esc>$gvd",
        after: "c def\n",
        cursor: (0, 0),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "xxU",
        after: "one two\n",
        cursor: (0, 0),
    },
    Case {
        text: "one two\n",
        at: (0, 0),
        keys: "xxUU",
        after: "e two\n",
        cursor: (0, 0),
    },
    // Paragraphs and sentences
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (0, 0),
        keys: "}",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (1, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (0, 0),
        keys: "}}",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (4, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (0, 0),
        keys: "2}",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (4, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (0, 0),
        keys: "}}}",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (5, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (5, 0),
        keys: "{",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (4, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (5, 0),
        keys: "{{",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (1, 0),
    },
    Case {
        text: "a\n\nb\nc\n\nd\n",
        at: (5, 0),
        keys: "{{{",
        after: "a\n\nb\nc\n\nd\n",
        cursor: (0, 0),
    },
    Case {
        text: "a b\nc\n\nd\n",
        at: (0, 2),
        keys: "d}",
        after: "a \n\nd\n",
        cursor: (0, 1),
    },
    Case {
        text: "One.  Two!\nThree? Four\n",
        at: (0, 0),
        keys: ")",
        after: "One.  Two!\nThree? Four\n",
        cursor: (0, 6),
    },
    Case {
        text: "One.  Two!\nThree? Four\n",
        at: (0, 0),
        keys: "))",
        after: "One.  Two!\nThree? Four\n",
        cursor: (1, 0),
    },
    Case {
        text: "One.  Two!\nThree? Four\n",
        at: (0, 0),
        keys: ")))",
        after: "One.  Two!\nThree? Four\n",
        cursor: (1, 7),
    },
    Case {
        text: "One\n\nTwo.\n",
        at: (0, 0),
        keys: ")",
        after: "One\n\nTwo.\n",
        cursor: (1, 0),
    },
    Case {
        text: "One\n\nTwo.\n",
        at: (0, 0),
        keys: "))",
        after: "One\n\nTwo.\n",
        cursor: (2, 0),
    },
    Case {
        text: "Hi there. Bye.\n",
        at: (0, 12),
        keys: "(",
        after: "Hi there. Bye.\n",
        cursor: (0, 10),
    },
    Case {
        text: "Hi there. Bye.\n",
        at: (0, 12),
        keys: "((",
        after: "Hi there. Bye.\n",
        cursor: (0, 0),
    },
];

/// The editor's text with `crlf` line ends after `replacements`, applied one after the other as the editor applies
/// them, or which of them reaches outside the text.
pub(crate) fn apply(text: &str, crlf: bool, replacements: &[Replacement]) -> Result<String, String> {
    let ending = if crlf { "\r\n" } else { "\n" };
    let mut text = text.to_string();
    for replacement in replacements {
        let offset = |(line, column): (usize, usize)| -> Option<usize> {
            let start = if line == 0 {
                0
            } else {
                text.match_indices('\n').nth(line - 1)?.0 + 1
            };
            let content = text[start..].split('\n').next()?;
            let content = content.strip_suffix('\r').unwrap_or(content);
            let mut within = content.char_indices().map(|(at, _)| at).chain([content.len()]);
            Some(start + within.nth(column)?)
        };
        let (Some(start), Some(end)) = (offset(replacement.start), offset(replacement.end)) else {
            return Err(format!("{replacement:?} outside {text:?}"));
        };
        text.replace_range(start..end, &replacement.text.replace('\n', ending));
    }
    Ok(text)
}

/// What `case` does to its text with `crlf` line ends, without the text's last line break with `cut`: a complaint
/// when the text, the cursor or the editor's text after the replacements are not Vim's.
fn run(case: &Case, crlf: bool, cut: bool) -> Option<String> {
    let trim = |text: &'static str| {
        if cut {
            text.strip_suffix('\n').unwrap_or(text)
        } else {
            text
        }
    };
    let (text, wanted) = (trim(case.text), trim(case.after));
    let ending = |text: &str| {
        if crlf {
            text.replace('\n', "\r\n")
        } else {
            text.to_string()
        }
    };
    let mut vim = Vim::new(&ending(text));
    vim.set_cursor(case.at.0, case.at.1);
    let done = vim.keys(case.keys);
    let (after, cursor) = (vim.text(), vim.cursor());
    let applied = apply(&ending(text), crlf, &done.replacements);
    let right = after == wanted && cursor == case.cursor && applied.as_deref() == Ok(&ending(wanted));
    (!right).then(|| {
        format!(
            "{:?} on {text:?}{}: {after:?} at {cursor:?}, the editor's {applied:?}, wanted {wanted:?} at {:?}",
            case.keys,
            if crlf { " (CRLF)" } else { "" },
            case.cursor
        )
    })
}

#[test]
fn every_operator_motion_and_text_object_does_what_vim_does_with_lf_and_crlf_and_without_a_last_line_break() {
    let wrong: Vec<String> = CASES
        .iter()
        .flat_map(|case| [(false, false), (true, false), (false, true), (true, true)].map(|(c, t)| run(case, c, t)))
        .flatten()
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn undo_and_redo_go_to_the_editor_and_ex_commands_to_the_app() {
    let mut vim = Vim::new("a\n");
    assert_eq!(vim.keys("u").effects, [Effect::Undo(1)]);
    assert_eq!(vim.keys("3u").effects, [Effect::Undo(3)]);
    assert_eq!(vim.keys("<C-R>").effects, [Effect::Redo(1)]);
    assert_eq!(vim.keys(":w<Enter>").effects, [Effect::Save]);
    assert_eq!(vim.keys(":q<Enter>").effects, [Effect::Close]);
    assert_eq!(vim.keys(":q!<Enter>").effects, [Effect::Discard]);
    assert_eq!(vim.keys(":wq<Enter>").effects, [Effect::SaveAndClose(true)]);
    assert_eq!(vim.keys(":x<Enter>").effects, [Effect::SaveAndClose(false)]);
    assert_eq!(vim.keys(":noh<Enter>").effects, [Effect::Highlight(None)]);
    assert_eq!(
        vim.keys(":vsplit<Enter>").effects,
        [Effect::Note(Note::UnsupportedCommand("vsplit".into()))]
    );
}

#[test]
fn a_search_that_fails_says_why_as_a_note() {
    let mut vim = Vim::new("abc\n");
    assert_eq!(vim.keys("/<Enter>").effects, [Effect::Note(Note::NoPreviousPattern)]);
    assert_eq!(
        vim.keys("/zz<Enter>").effects,
        [Effect::Note(Note::PatternNotFound("zz".into()))]
    );
    assert!(matches!(
        vim.keys(":s/(/x/<Enter>").effects.as_slice(),
        [Effect::Note(Note::InvalidPattern { pattern, .. })] if pattern == "("
    ));
}

#[test]
fn the_modes_show_as_the_status_bar_names_them() {
    let mut vim = Vim::new("abc\n");
    assert_eq!(vim.mode(), Mode::Normal);
    vim.keys("i");
    assert_eq!(vim.mode(), Mode::Insert);
    vim.keys("<Esc>v");
    assert_eq!(vim.mode(), Mode::Visual);
    vim.keys("<Esc>V");
    assert_eq!(vim.mode(), Mode::VisualLine);
    vim.keys("<Esc><C-V>");
    assert_eq!(vim.mode(), Mode::VisualBlock);
    vim.keys("<Esc>R");
    assert_eq!(vim.mode(), Mode::Replace);
    vim.keys("<Esc>/");
    assert_eq!(vim.mode(), Mode::Prompt);
}

#[test]
fn a_change_from_outside_is_taken_without_losing_the_cursor() {
    let mut vim = Vim::new("one\ntwo\n");
    vim.set_cursor(1, 2);
    vim.resync("one\ntwo\nthree\n");
    assert_eq!(vim.cursor(), (1, 2));
    vim.resync("x\n");
    assert_eq!(vim.cursor(), (0, 1));
}

#[test]
fn a_rope_from_outside_reads_as_its_text_does() {
    let cases = ["", "one", "one\ntwo\n", "a\r\nb", "a\rb\r", "x\r\n\r\n", "lone cr\r"];
    for text in cases {
        let (mut by_text, mut by_rope) = (Vim::new("start\n"), Vim::new("start\n"));
        let found = by_text.resync(text);
        assert_eq!(by_rope.resync_rope(&ropey::Rope::from_str(text)), found, "{text:?}");
        assert_eq!(by_rope.text(), by_text.text(), "{text:?}");
        assert_eq!(
            super::has_crlf(&ropey::Rope::from_str(text)),
            text.contains("\r\n"),
            "{text:?}"
        );
    }
    // A CRLF in a rope of many leaves.
    let long = format!("{}\r\n{}", "a".repeat(5000), "b".repeat(5000));
    let rope = ropey::Rope::from_str(&long);
    assert!(super::has_crlf(&rope));
    let mut vim = Vim::new("x\n");
    vim.resync_rope(&rope);
    assert_eq!(vim.text(), long.replace("\r\n", "\n"));
}

/// The gate of E104: a 10 MB file stays within the keystroke budget of E25 with the copy, motions and an edit.
#[test]
#[ignore = "a timing budget; make perf runs it"]
fn a_ten_megabyte_file_keeps_a_keystroke_under_sixteen_milliseconds() {
    let line = "    let value = compute(argument, other_argument); // a comment\n";
    let text: String = line.repeat(10 * 1024 * 1024 / line.len());
    // The first build pays for memory the process has not touched yet; the median of a few is what a user sees.
    let builds: Vec<Duration> = (0..5)
        .map(|_| {
            let built = Instant::now();
            drop(Vim::new(&text));
            built.elapsed()
        })
        .collect();
    let mut vim = Vim::new(&text);
    vim.set_cursor(80_000, 0);
    let mut slowest = Duration::ZERO;
    for keys in ["w", "w", "b", "j", "k", "x", "dd", "u"] {
        let started = Instant::now();
        vim.keys(keys);
        slowest = slowest.max(started.elapsed());
    }

    term_base::perf::durations("term-vim: building the copy of 10 MB", &builds);
    term_base::perf::durations("term-vim: the slowest key in 10 MB", &[slowest]);
    eprintln!("copy of 10 MB built in {builds:?}, slowest key {slowest:?}");
    assert!(slowest < Duration::from_millis(16), "slowest key {slowest:?}");
}

#[test]
fn indenting_goes_to_the_editor_with_the_lines_it_reaches() {
    let mut vim = Vim::new("a\nb\nc\nd\n");
    vim.set_cursor(1, 0);
    assert_eq!(vim.keys(">>").effects, [Effect::Indent(1..2, 1)]);
    assert_eq!(vim.keys("2<<").effects, [Effect::Indent(1..3, -1)]);
    vim.set_cursor(0, 0);
    assert_eq!(vim.keys("Vj>").effects, [Effect::Indent(0..2, 1)]);
    assert_eq!(vim.mode(), Mode::Normal);
}

#[test]
fn the_plus_register_and_a_shared_unnamed_one_are_the_clipboard() {
    let mut vim = Vim::new("one\ntwo\n");
    assert_eq!(vim.keys("\"+yy").effects, [Effect::Copy("one\n".into())]);
    assert!(vim.keys("yy").effects.is_empty(), "the unnamed register is Vim's own");
    vim.set_clipboard("from mac");
    vim.keys("\"+p");
    assert_eq!(vim.text(), "ofrom macne\ntwo\n");
    let mut shared = Vim::new("one\n");
    shared.share_clipboard(true);
    assert_eq!(shared.keys("yw").effects, [Effect::Copy("one".into())]);
    shared.set_clipboard("X");
    shared.keys("P");
    assert_eq!(shared.text(), "Xone\n");
}
