use super::*;
use crate::{Document, Edit};

/// A small random generator, so that the tests need no crate and give the same inputs every run.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(n.max(1)).unwrap()).unwrap()
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const PIECES: [&str; 38] = [
    "fn ", "def ", "'", "\"", "\"\"\"", "'''", "`", "#", "//", "/*", "*/", "<", ">", "<!--", "-->", "--", "%%", "1",
    "ä", "😀", "\n", "\n", "\r\n", "\r", "\\", "x = ", "[t]", "{", "}", "(", ")", "[", "]", "key: ", " ", "true", "\t",
    "a",
];

/// The colours `tokens` gives the whole text, cut into lines as the editor shows them.
fn whole_text(language: Language, text: &str) -> Vec<Vec<Span>> {
    let lines = Text::new(text);
    let tokens = splat_markdown::code::tokens(language.name(), text);
    let column = |byte| u32::try_from(lines.point(byte).unwrap().utf16).unwrap();
    (0..lines.line_count())
        .map(|n| {
            let content = lines.line_content(n).unwrap();
            tokens
                .iter()
                .filter_map(|(range, kind)| {
                    let (from, to) = (range.start.max(content.start), range.end.min(content.end));
                    (from < to).then(|| Span {
                        start: column(from),
                        end: column(to),
                        kind: *kind,
                    })
                })
                .collect()
        })
        .collect()
}

fn all_spans(document: &Document) -> Vec<Vec<Span>> {
    (0..document.text().line_count())
        .map(|n| document.spans(n).to_vec())
        .collect()
}

fn lexed(language: Language, text: &str) -> Document {
    let mut document = Document::untitled(text);
    document.set_language(Some(language));
    document.highlight(0..usize::MAX);
    document
}

fn has_lone_cr(text: &str) -> bool {
    text.match_indices('\r')
        .any(|(at, _)| !text[at + 1..].starts_with('\n'))
}

#[test]
fn lines_lexed_one_by_one_colour_as_the_whole_text_does() {
    for (language, text) in [
        ("rust", "fn a() { /* one\ntwo */ let s = \"x\\\ny\"; }\n// end"),
        (
            "python",
            "def f():\n    '''doc\n    more'''\n    return \"\"\"a\nb\"\"\" # c\n",
        ),
        ("html", "<p class=\"a\n b\">text <!-- c\nd --> <b\n id='x'>y</b>\n"),
        (
            "javascript",
            "let t = `a\n${b}\nc`; // x\nconst n = 1;\r\nlet u = 'q';\r\n",
        ),
        ("toml", "[a]\nx = \"\"\"\nk = 1\n\"\"\" y = 2\nz = '''\n'''\n"),
        ("yaml", "a: \"b\\\nc\" d: e\nf: g\n"),
        ("go", "x := `raw\n` + \"s\"\n/* c */ y := 2\n"),
        ("sql", "SELECT /* a\nb */ x -- y\nFROM t\n"),
    ] {
        let language = Language::named(language).unwrap();
        assert_eq!(
            all_spans(&lexed(language, text)),
            whole_text(language, text),
            "{} {text:?}",
            language.name()
        );
    }
}

#[test]
fn random_edits_keep_every_line_as_lexing_the_whole_text_would() {
    let mut rng = Rng(0x5eed_1e6e_d0c5);
    for language in Language::all() {
        for _ in 0..40 {
            let text: String = (0..rng.below(40)).map(|_| rng.pick(&PIECES)).collect();
            let mut document = Document::untitled(&text);
            document.set_language(Some(language));
            document.highlight(0..usize::MAX);
            for _ in 0..8 {
                let len = document.text().len_utf16();
                let a = rng.below(len + 1);
                let b = (a + rng.below(6)).min(len);
                let inserted: String = (0..rng.below(4)).map(|_| rng.pick(&PIECES)).collect();
                let edit = Edit {
                    range_utf16: a..b,
                    text: inserted,
                };
                if document.apply(edit).is_err() {
                    continue;
                }
                let first = rng.below(document.text().line_count());
                document.highlight(first..first + 3);
                document.highlight(0..usize::MAX);
                let now = document.text().to_string();
                assert_eq!(
                    all_spans(&document),
                    all_spans(&lexed(language, &now)),
                    "{} {now:?}",
                    language.name()
                );
                if !has_lone_cr(&now) {
                    assert_eq!(
                        all_spans(&document),
                        whole_text(language, &now),
                        "{} {now:?}",
                        language.name()
                    );
                }
            }
        }
    }
}

#[test]
fn edits_made_together_leave_the_lines_one_splice_per_edit_would() {
    let mut rng = Rng(0x5911_ce11_7e57);
    for round in 0..2_000 {
        let length = rng.below(40);
        let mut together: Vec<Option<usize>> = (0..length).map(Some).collect();
        let mut one_by_one = together.clone();
        let mut edits = Vec::new();
        let mut before = length + 2;
        for _ in 0..rng.below(8) {
            // Mostly towards the start, as the edits of a new text run, and now and then not.
            let first = if rng.below(5) == 0 {
                rng.below(length + 3)
            } else {
                rng.below(before + 1)
            };
            before = first;
            edits.push(LineEdit {
                first,
                removed: rng.below(4),
                added: rng.below(4),
            });
        }
        for edit in &edits {
            let end = (edit.first + edit.removed).min(one_by_one.len());
            one_by_one.splice(edit.first.min(end)..end, std::iter::repeat_n(None, edit.added));
        }
        splice_lines(&mut together, &edits);
        assert_eq!(together, one_by_one, "round {round}: {edits:?}");
    }
}

#[test]
fn a_new_text_with_many_changes_keeps_every_line_as_lexing_it_anew_would() {
    let mut rng = Rng(0x0dd5_eed5_7e47);
    for language in Language::all() {
        for _ in 0..20 {
            let pieces: Vec<&str> = (0..rng.below(80)).map(|_| rng.pick(&PIECES)).collect();
            let mut changed = pieces.clone();
            for _ in 0..rng.below(12) {
                let at = rng.below(changed.len() + 1);
                let piece = rng.pick(&PIECES);
                match rng.below(3) {
                    0 => changed.insert(at, piece),
                    1 if at < changed.len() => {
                        changed.remove(at);
                    }
                    _ if at < changed.len() => changed[at] = piece,
                    _ => {}
                }
            }
            let (old, new) = (pieces.concat(), changed.concat());
            let mut document = Document::untitled(&old);
            document.set_language(Some(language));
            document.highlight(0..usize::MAX);
            let _ = document.folds(4);
            document.replace_all(&new).unwrap();
            document.highlight(0..usize::MAX);
            assert_eq!(document.text().to_string(), new);
            assert_eq!(
                all_spans(&document),
                all_spans(&lexed(language, &new)),
                "{} {old:?} → {new:?}",
                language.name()
            );
            assert_eq!(
                document.folds(4),
                super::folding::indentation_folds(&Text::new(&new), 4),
                "{old:?} → {new:?}"
            );
        }
    }
}

#[test]
fn a_keystroke_relexes_its_line_and_an_opened_comment_runs_to_its_end() {
    let rust = Language::named("rust").unwrap();
    let text: String = (0..200).map(|n| format!("let x{n} = {n}; // c\n")).collect();
    let mut document = lexed(rust, &text);
    let start = document.text().offset(100, 4, crate::Unit::Utf16).unwrap();
    let at = document.text().byte_to_utf16(start).unwrap();
    document
        .apply(Edit {
            range_utf16: at..at,
            text: "y".into(),
        })
        .unwrap();
    assert_eq!(document.highlight(90..110), Some(100..101));
    document
        .apply(Edit {
            range_utf16: 0..0,
            text: "/*".into(),
        })
        .unwrap();
    assert_eq!(document.highlight(0..40), Some(0..40));
    assert!(document.spans(39).iter().all(|s| s.kind == CodeToken::Comment));
    assert_eq!(
        document.highlight_more(std::time::Duration::from_secs(5)),
        Some(40..200)
    );
    assert!(document.is_highlighted());
}

#[test]
fn a_range_far_below_is_guessed_first_and_corrected_once_lexing_reaches_it() {
    let rust = Language::named("rust").unwrap();
    let mut text = String::from("/* open\n");
    text.extend((0..CATCH_UP_LINES * 2).map(|n| format!("let x{n} = 1;\n")));
    text.push_str("*/\n");
    let mut document = Document::untitled(&text);
    document.set_language(Some(rust));
    let far = CATCH_UP_LINES + 100;
    assert_eq!(document.highlight(far..far + 5), Some(far..far + 5));
    assert!(!document.is_highlighted());
    assert!(
        document.spans(far).iter().any(|s| s.kind == CodeToken::Keyword),
        "guessed: outside the comment"
    );
    while !document.is_highlighted() {
        document.highlight_more(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        document.spans(far),
        [Span {
            start: 0,
            end: 14,
            kind: CodeToken::Comment
        }]
    );
}

#[test]
fn catching_up_within_a_budget_guesses_the_lines_it_did_not_reach() {
    let rust = Language::named("rust").unwrap();
    let mut text = String::from("/* open\n");
    text.extend((0..CATCH_UP_LINES).map(|n| format!("let x{n} = 1;\n")));
    text.push_str("*/\n");
    let mut document = Document::untitled(&text);
    document.set_language(Some(rust));
    let near = CATCH_UP_LINES - 100;
    assert!(
        document
            .highlight_within(near..near + 5, std::time::Duration::ZERO)
            .is_some_and(|changed| changed.contains(&near))
    );
    assert!(
        document.spans(near).iter().any(|s| s.kind == CodeToken::Keyword),
        "guessed: outside the comment"
    );
    while !document.is_highlighted() {
        document.highlight_more(std::time::Duration::from_millis(50));
    }
    assert!(document.spans(near).iter().all(|s| s.kind == CodeToken::Comment));
}

#[test]
fn a_long_first_line_of_a_comment_shortens_to_its_ends() {
    let rust = Language::named("rust").unwrap();
    let long = "x".repeat(LONG_OPENING * 2);
    let text = format!("let a = 1; /* {long}\nstill */ let b = 2;\n");
    let lines = Text::new(&text);
    let mut syntax = Syntax::new(Some(rust), lines.line_count());
    syntax.highlight(&lines, 0..3, None);
    let open = syntax.lines[0].as_ref().unwrap().end.open.clone().unwrap();
    assert!(open.len() < 40, "{open:?}");
    assert_eq!(all_spans(&lexed(rust, &text)), whole_text(rust, &text));
}

#[test]
fn brackets_pair_across_lines_past_strings_and_comments() {
    let rust = Language::named("rust").unwrap();
    let text = "fn a() {\n    let s = \"}\"; // }\n    { x(\"(\") }\n}\n";
    let mut document = lexed(rust, text);
    let open = text.find('{').unwrap();
    let close = text.rfind('}').unwrap();
    let pair = Some(BracketPair { open, close });
    assert_eq!(document.matching_bracket(open), pair, "the bracket after the caret");
    assert_eq!(
        document.matching_bracket(open + 1),
        pair,
        "the bracket before the caret"
    );
    assert_eq!(document.matching_bracket(close), pair, "from the closing one");
    let inner = text.find("{ x").unwrap();
    assert_eq!(
        document.matching_bracket(inner),
        Some(BracketPair {
            open: inner,
            close: text.find(" }\n}").unwrap() + 1
        })
    );
    assert_eq!(
        document.matching_bracket(text.find("\"}\"").unwrap() + 1),
        None,
        "inside a string"
    );
    let mut unbalanced = lexed(rust, "( [ )");
    assert_eq!(unbalanced.matching_bracket(0), Some(BracketPair { open: 0, close: 4 }));
    assert_eq!(unbalanced.matching_bracket(2), None);
}

#[test]
fn plain_text_has_no_colours_but_brackets() {
    let mut document = Document::untitled("a (b\nc) \"(\"");
    assert_eq!(document.language(), None);
    document.highlight(0..2);
    assert!(document.spans(0).is_empty());
    assert_eq!(document.matching_bracket(2), Some(BracketPair { open: 2, close: 6 }));
}

#[test]
fn spans_count_utf16_columns() {
    let python = Language::named("python").unwrap();
    let document = lexed(python, "x = '😀' # ä\n");
    assert_eq!(
        document.spans(0),
        [
            Span {
                start: 4,
                end: 8,
                kind: CodeToken::String
            },
            Span {
                start: 9,
                end: 12,
                kind: CodeToken::Comment
            }
        ]
    );
}

/// A document whose second line is too long to lex, between two short lines of Rust.
fn with_a_very_long_line() -> Document {
    let rust = Language::named("rust").unwrap();
    let long = "x(\"y\") + ".repeat(LONGEST_LEXED_LINE);
    lexed(rust, &format!("let a = 1;\n{long}\nlet b = 2;\n"))
}

/// Types twenty letters into the long line of `with_a_very_long_line`, highlighting the screen after each.
fn type_into_the_long_line(document: &mut Document) {
    let at = document
        .text()
        .byte_to_utf16(document.text().line_bytes(1).unwrap().start + 3)
        .unwrap();
    for _ in 0..20 {
        document
            .apply(Edit {
                range_utf16: at..at,
                text: "z".into(),
            })
            .unwrap();
        document.highlight(0..3);
    }
}

#[test]
fn a_very_long_line_stays_plain_while_typing_in_it() {
    let mut document = with_a_very_long_line();
    assert!(document.spans(1).is_empty());
    assert_eq!(document.spans(2).first().map(|s| s.kind), Some(CodeToken::Keyword));
    type_into_the_long_line(&mut document);
    assert!(document.spans(1).is_empty());
    assert_eq!(document.spans(2).first().map(|s| s.kind), Some(CodeToken::Keyword));
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn budget_typing_in_a_very_long_line_stays_cheap() {
    let mut times: Vec<_> = (0..5)
        .map(|_| {
            let mut document = with_a_very_long_line();
            let start = std::time::Instant::now();
            type_into_the_long_line(&mut document);
            start.elapsed()
        })
        .collect();
    term_base::perf::durations("term-document: typing in a very long line", &times);
    times.sort();
    let median = times[times.len() / 2];
    assert!(median < std::time::Duration::from_millis(100), "{median:?}");
}

#[test]
fn files_of_code_are_coloured_by_their_name() {
    let scratch = term_base::testing::Scratch::new("colours");
    let files = [
        ("main.rs", "// note\nfn main() { let s = \"x\"; }\n", CodeToken::Comment),
        ("Cargo.toml", "[package]\nname = \"tapa\" # note\n", CodeToken::String),
        ("data.json", "{\n  \"on\": true\n}\n", CodeToken::Keyword),
    ];
    for (name, text, kind) in files {
        let path = scratch.path().join(name);
        std::fs::write(&path, text).unwrap();
        let mut document = Document::open(&path).unwrap();
        assert!(document.language().is_some(), "{name}");
        let _ = document.highlight_within(0..document.text().line_count(), std::time::Duration::from_secs(1));
        let kinds: Vec<_> = all_spans(&document).concat().iter().map(|span| span.kind).collect();
        assert!(kinds.contains(&kind), "{name}: {kinds:?}");
    }
}
