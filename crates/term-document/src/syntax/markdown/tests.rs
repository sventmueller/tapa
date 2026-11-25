use std::time::Duration;

use super::*;
use crate::syntax::Language;
use crate::{Document, Edit};

/// Every kind of Markdown splat's source view colours.
const SAMPLE: &str = "---\ntitle: Notes\n---\n\n# Heading *one*\n\nSome **bold**, _italic_, ~~gone~~ and `code` with \
                      [a link](https://example.org).\n\n> quoted\n\n- item\n- [ ] task\n\n| a | b |\n|---|---|\n| 1 | 2 \
                      |\n\n```rust\nfn main() { let x = \"s\"; }\n```\n\n<div>html</div>\n\n$$\nx^2\n$$\n";

/// splat's styles of the whole text, cut into lines as the editor shows them.
fn whole_text(text: &str) -> Vec<Vec<Styled>> {
    let lines = Text::new(text);
    let spans = splat_markdown::incremental::parse_and_color(text).spans;
    let column = |byte| u32::try_from(lines.point(byte).unwrap().utf16).unwrap();
    (0..lines.line_count())
        .map(|n| {
            let content = lines.line_content(n).unwrap();
            let first = spans.partition_point(|(range, _)| range.end.get() <= content.start);
            spans[first..]
                .iter()
                .take_while(|(range, _)| range.start.get() < content.end)
                .filter_map(|(range, style)| {
                    let (from, to) = (range.start.get().max(content.start), range.end.get().min(content.end));
                    (from < to).then(|| Styled {
                        start: column(from),
                        end: column(to),
                        style: *style,
                    })
                })
                .collect()
        })
        .collect()
}

fn all_styles(document: &Document) -> Vec<Vec<Styled>> {
    (0..document.text().line_count())
        .map(|n| document.styles(n).to_vec())
        .collect()
}

fn opened(text: &str) -> Document {
    let mut document = Document::untitled(text);
    document.set_language(Some(Language::MARKDOWN));
    document.highlight(0..usize::MAX);
    document
}

fn tokens(document: &Document, line: usize) -> Vec<Token> {
    document.styles(line).iter().filter_map(|s| s.style.token).collect()
}

fn insert(document: &mut Document, line: usize, column: usize, text: &str) {
    let byte = document.text().offset(line, column, crate::Unit::Utf16).unwrap();
    let at = document.text().byte_to_utf16(byte).unwrap();
    document
        .apply(Edit {
            range_utf16: at..at,
            text: text.into(),
        })
        .unwrap();
}

#[test]
fn every_line_gets_the_styles_splats_source_view_gives_the_whole_text() {
    let document = opened(SAMPLE);
    assert_eq!(all_styles(&document), whole_text(SAMPLE));
    assert!(document.is_highlighted());
    assert!(
        document.styles(4).iter().all(|s| s.style.bold) && tokens(&document, 4).contains(&Token::Heading),
        "{:?}",
        document.styles(4)
    );
    assert!(tokens(&document, 1).contains(&Token::Comment), "front matter");
    let link_line = tokens(&document, 6);
    for token in [Token::Code, Token::LinkText, Token::Url, Token::Punctuation] {
        assert!(link_line.contains(&token), "{token:?} in {link_line:?}");
    }
    let flags = |line: usize| document.styles(line).iter().map(|s| s.style).collect::<Vec<_>>();
    assert!(flags(6).iter().any(|s| s.italic) && flags(6).iter().any(|s| s.strike));
    assert!(tokens(&document, 8).contains(&Token::Quote));
    assert!(tokens(&document, 11).contains(&Token::ListMarker));
    assert!(tokens(&document, 14).contains(&Token::Punctuation), "a table's rule");
    let code = tokens(&document, 18);
    assert!(
        code.contains(&Token::Keyword) && code.contains(&Token::String),
        "a fenced block in its language's colours: {code:?}"
    );
    assert!(tokens(&document, 21).contains(&Token::Html));
    assert!(tokens(&document, 24).contains(&Token::Math));
    assert!(
        document.spans(18).is_empty(),
        "Markdown has no colours of a language of code"
    );
}

#[test]
fn a_keystroke_in_a_paragraph_restyles_its_line_and_an_opened_fence_the_lines_below() {
    let text: String = (0..300)
        .map(|n| format!("Paragraph {n} with *emphasis*.\n\n"))
        .collect();
    let mut document = opened(&text);
    insert(&mut document, 200, 0, "x");
    assert_eq!(document.highlight(190..210), Some(200..201));
    assert_eq!(
        document.highlight_more(Duration::from_secs(5)),
        None,
        "nothing else changed"
    );
    assert!(document.is_highlighted());
    insert(&mut document, 100, 0, "```\n");
    let shown = document.highlight(90..120).expect("the lines on screen change");
    assert!(shown.start <= 100 && shown.end == 120, "{shown:?}");
    let code = document.styles(111);
    assert!(
        !code.is_empty() && code.iter().all(|s| s.style.token == Some(Token::Code)),
        "{code:?}"
    );
    assert!(!document.is_highlighted(), "the lines below wait for the next slice");
    assert_eq!(
        document.highlight_more(Duration::from_secs(5)),
        Some(121..600),
        "the paragraphs below, now code"
    );
    assert!(document.is_highlighted());
    assert_eq!(all_styles(&document), whole_text(&document.text().to_string()));
}

#[test]
fn random_edits_keep_every_line_as_parsing_the_whole_text_would() {
    const PIECES: [&str; 24] = [
        "# ", "*", "**", "_", "`", "```", "~~", "[a](b)", "[x]: y", "<b>", "</b>", "> ", "- ", "1. ", "| ", "---",
        "$$", "\n", "\n\n", "ä", "😀", "word ", "\t", "\\",
    ];
    let mut seed = 0x6d61_726b_646f_776eu64;
    let mut below = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % u64::try_from(n.max(1)).unwrap()).unwrap()
    };
    for _ in 0..60 {
        let text: String = (0..below(60)).map(|_| PIECES[below(PIECES.len())]).collect();
        let mut document = opened(&text);
        let mut shown = all_styles(&document);
        for _ in 0..10 {
            let len = document.text().len_utf16();
            let a = below(len + 1);
            let b = (a + below(6)).min(len);
            let inserted: String = (0..below(4)).map(|_| PIECES[below(PIECES.len())]).collect();
            let first = document
                .text()
                .utf16_to_byte(a)
                .map(|byte| document.text().line_of(byte));
            let removed = document
                .text()
                .utf16_to_byte(b)
                .map(|byte| document.text().line_of(byte));
            let (Ok(first), Ok(last)) = (first, removed) else {
                continue;
            };
            let edit = Edit {
                range_utf16: a..b,
                text: inserted.clone(),
            };
            if document.apply(edit).is_err() {
                continue;
            }
            // What the editor shows: the edited lines and every line reported changed are painted again.
            let added = inserted.matches('\n').count() + 1;
            shown.splice(first..=last, std::iter::repeat_n(Vec::new(), added));
            let mut repaint: Vec<usize> = (first..first + added).collect();
            let at = below(document.text().line_count());
            repaint.extend(document.highlight(at..at + 3).into_iter().flatten());
            while !document.is_highlighted() {
                repaint.extend(document.highlight_more(Duration::from_secs(1)).into_iter().flatten());
            }
            for n in repaint {
                if let Some(line) = shown.get_mut(n) {
                    *line = document.styles(n).to_vec();
                }
            }
            let now = document.text().to_string();
            assert_eq!(shown, whole_text(&now), "{now:?}");
        }
    }
}

#[test]
fn a_long_text_is_parsed_on_a_thread_and_its_start_meanwhile() {
    let section = "## Part {n}\n\nText with **bold** and `code`.\n\n```rust\nfn f() {}\n```\n\n";
    let text: String = (0..2200).map(|n| section.replace("{n}", &n.to_string())).collect();
    assert!(text.len() > PARSED_AT_ONCE);
    let mut document = Document::untitled(&text);
    document.set_language(Some(Language::MARKDOWN));
    assert_eq!(document.highlight(0..60), Some(0..60), "the start is coloured at once");
    assert!(tokens(&document, 0).contains(&Token::Heading));
    let far = document.text().line_count() - 10;
    insert(&mut document, 2, 0, "Edited while parsing. ");
    assert!(
        document.styles(far).is_empty(),
        "beyond the start nothing is known before the parse is done"
    );
    assert!(!document.is_highlighted());
    document.highlight(far..far + 5);
    highlight_through(&mut document);
    assert_eq!(all_styles(&document), whole_text(&document.text().to_string()));
}

fn highlight_through(document: &mut Document) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !document.is_highlighted() {
        assert!(std::time::Instant::now() < deadline, "the parse never arrived");
        document.highlight_more(Duration::from_millis(4));
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_fence_that_restyles_a_long_text_is_parsed_on_a_thread_and_the_lines_keep_their_styles_meanwhile() {
    let text: String = (0..4000)
        .map(|n| format!("## Part {n}\n\nText with **bold** and `code`.\n\n"))
        .collect();
    assert!(text.len() > PARSED_AT_ONCE);
    let mut document = Document::untitled(&text);
    document.set_language(Some(Language::MARKDOWN));
    document.highlight(0..60);
    highlight_through(&mut document);
    let far = document.text().line_count() - 9;
    let before = document.styles(far).to_vec();
    assert!(before.iter().any(|s| s.style.bold), "{before:?}");
    insert(&mut document, 0, 0, "```\n");
    document.highlight(far..far + 5);
    assert_eq!(
        document.styles(far + 1),
        &before[..],
        "the old styles stay while the parse runs"
    );
    assert!(!document.is_highlighted());
    highlight_through(&mut document);
    assert!(tokens(&document, far + 1).iter().all(|t| *t == Token::Code));
    assert_eq!(all_styles(&document), whole_text(&document.text().to_string()));
}

#[test]
fn the_kept_copy_follows_edits_inside_line_breaks_and_new_texts_of_few_and_many_edits() {
    let check = |document: &mut Document| {
        document.highlight(0..usize::MAX);
        let now = document.text().to_string();
        assert_eq!(all_styles(document), whole_text(&now), "{now:?}");
    };
    let mut document = opened("# One\r\n\r\nSome *text*\r\n\r\n```\r\ncode\r\n```\r\n");
    // 6..6 lies between the CR and the LF of the first line break, which the document widens over both.
    for (range, new) in [(5..6, "x"), (6..6, "**"), (0..2, ""), (3..3, "ä😀")] {
        document
            .apply(Edit {
                range_utf16: range,
                text: new.into(),
            })
            .unwrap();
        check(&mut document);
    }
    document
        .replace_all("# One\r\n\r\nSome _text_\r\n\r\n```\r\ncode\r\n```\r\n")
        .unwrap();
    check(&mut document);
    let many: String = (0..40).map(|n| format!("- item {n}\n\n")).collect();
    document.replace_all(&many).unwrap();
    check(&mut document);
    document.replace_all(&many.replace("item 3", "**item** 3")).unwrap();
    check(&mut document);
    let far_apart = many
        .replacen("item 2\n", "*item* 2\n", 1)
        .replacen("item 38", "item `38`", 1);
    document.replace_all(&far_apart).unwrap();
    check(&mut document);
}

#[test]
fn edits_merge_into_one_that_turns_the_parsed_text_into_the_text_now() {
    let mut seed = 0x6d65_7267_6564_6974u64;
    let mut below = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % u64::try_from(n.max(1)).unwrap()).unwrap()
    };
    for _ in 0..500 {
        let parsed: String = (0..below(40))
            .map(|n| char::from(b'a' + u8::try_from(n % 26).unwrap()))
            .collect();
        let mut now = parsed.clone();
        let mut since = Since::Unchanged;
        for _ in 0..=below(MOST_SPLICES) {
            let start = below(now.len() + 1);
            let end = start + below(now.len() - start + 1);
            let added = &"XYZ"[..below(4)];
            now.replace_range(start..end, added);
            since.merge(&(start..end), added.len());
        }
        let Since::Edit { old, new_end } = since else {
            panic!("{since:?}");
        };
        let rebuilt = format!(
            "{}{}{}",
            &parsed[..old.start],
            &now[old.start..new_end],
            &parsed[old.end..]
        );
        assert_eq!(rebuilt, now, "{parsed:?} -> {old:?}, {new_end}");
    }
}
