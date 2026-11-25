//! Properties of edits, positions and the three-way merge, checked against simple models over generated texts (E96).
//! The seed is fixed, so two runs of `make check` try the same cases; `make fuzz ARGS=properties` sets
//! `PROPTEST_RNG_SEED` and `PROPTEST_CASES`, which proptest reads over it. A failure is kept in
//! `properties.proptest-regressions` beside this file.

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use term_document::merge::{self, Reconciliation};
use term_document::{Document, Edit, Text, Unit};

fn config() -> Config {
    Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x00da_6e0d),
        ..Config::default()
    }
}

/// Text from pieces that differ in every unit and in line breaks: ASCII, a two-byte letter, an emoji outside the
/// BMP, a combining mark and the three line breaks.
fn text() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("a"),
        Just("xyz"),
        Just("ä"),
        Just("😀"),
        Just("e\u{301}"),
        Just("\n"),
        Just("\r\n"),
        Just("\r"),
        Just("\t"),
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
}

/// The UTF-16 offset of each character boundary of `text`, with its byte offset.
fn boundaries(text: &str) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 0)];
    let mut utf16 = 0;
    for (byte, c) in text.char_indices() {
        utf16 += c.len_utf16();
        out.push((utf16, byte + c.len_utf8()));
    }
    out
}

/// An edit of `text` between two of its character boundaries that `a` and `b` pick, and the reference result.
fn edit_of(text: &str, a: usize, b: usize, new: &str) -> (Edit, String) {
    let places = boundaries(text);
    let (mut from, mut to) = (places[a % places.len()], places[b % places.len()]);
    if from > to {
        std::mem::swap(&mut from, &mut to);
    }
    let expected = format!("{}{new}{}", &text[..from.1], &text[to.1..]);
    let edit = Edit {
        range_utf16: from.0..to.0,
        text: new.to_string(),
    };
    (edit, expected)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn edits_make_the_text_a_string_would_and_their_inverse_restores_it(
        start in text(),
        steps in proptest::collection::vec((any::<usize>(), any::<usize>(), text()), 1..8),
    ) {
        let mut document = Document::untitled(&start);
        let mut model = start;
        for (a, b, new) in steps {
            let before = model.clone();
            let (edit, expected) = edit_of(&model, a, b, &new);
            let change = document.apply(edit).expect("an edit between character boundaries");
            model = expected;
            prop_assert_eq!(document.text().to_string(), model.clone());
            prop_assert_eq!(document.revision(), change.revision);
            prop_assert!(change.bytes.end <= before.len());
            prop_assert_eq!(&model[change.new_bytes()], change.text.as_str());
            prop_assert_eq!(document.text().point(change.new_bytes().end).ok(), Some(change.new_end));

            let undo = Edit {
                range_utf16: change.new_utf16(),
                text: before[change.bytes.clone()].to_string(),
            };
            let mut copy = Document::untitled(&model);
            copy.apply(undo).expect("the inverse of an edit");
            prop_assert_eq!(copy.text().to_string(), before);
        }
    }

    #[test]
    fn an_edit_inside_a_character_is_refused_and_changes_nothing(
        start in (text(), text()).prop_map(|(a, b)| format!("{a}😀{b}")),
        at in any::<usize>(),
    ) {
        let places: Vec<usize> = start
            .chars()
            .scan(0, |utf16, c| {
                let inside = (c.len_utf16() == 2).then_some(*utf16 + 1);
                *utf16 += c.len_utf16();
                Some(inside)
            })
            .flatten()
            .collect();
        let inside = places[at % places.len()];
        let mut document = Document::untitled(&start);
        let edit = Edit { range_utf16: inside..inside, text: "x".into() };
        prop_assert!(document.apply(edit).is_err());
        prop_assert_eq!(document.text().to_string(), start);
        prop_assert_eq!(document.revision(), term_document::Revision::FIRST);
    }

    #[test]
    fn replacing_everything_ends_at_the_new_text_with_edits_that_hold_in_order(old in text(), new in text()) {
        let mut document = Document::untitled(&old);
        let changes = document.replace_all(&new).expect("a document that is not read-only");
        prop_assert_eq!(document.text().to_string(), new.clone());
        let mut model = old;
        for change in &changes {
            model.replace_range(change.bytes.clone(), &change.text);
        }
        prop_assert_eq!(model, new);
    }

    #[test]
    fn every_place_reads_back_in_every_unit(content in text()) {
        let text = Text::new(&content);
        for (utf16, byte) in boundaries(&content) {
            prop_assert_eq!(text.byte_to_utf16(byte), Ok(utf16));
            prop_assert_eq!(text.utf16_to_byte(utf16), Ok(byte));
            let point = text.point(byte).expect("a character boundary");
            let in_line_break = content[..byte].ends_with('\r') && content[byte..].starts_with('\n');
            if in_line_break {
                continue;
            }
            for unit in [Unit::Utf8, Unit::Utf16, Unit::Utf32] {
                let column = point.column(unit);
                prop_assert_eq!(text.offset(point.line, column, unit), Ok(byte));
                let (line, character) = (u32::try_from(point.line).unwrap(), u32::try_from(column).unwrap());
                prop_assert_eq!(text.lsp_offset(line, character, unit), byte);
            }
        }
    }

    #[test]
    fn a_server_position_always_lands_on_a_character_boundary(
        content in text(),
        line in 0u32..50,
        character in prop_oneof![0u32..80, Just(u32::MAX)],
    ) {
        let text = Text::new(&content);
        for unit in [Unit::Utf8, Unit::Utf16, Unit::Utf32] {
            let byte = text.lsp_offset(line, character, unit);
            prop_assert!(content.is_char_boundary(byte), "{byte} in {content:?}");
        }
    }

    #[test]
    fn changes_in_different_places_merge_into_both(
        size in 5usize..30,
        a in any::<usize>(),
        a_len in 0usize..4,
        gap in 1usize..4,
        b_len in 0usize..4,
        buffer_new in 0usize..4,
        disk_new in 0usize..4,
    ) {
        let base: Vec<String> = (0..size).map(|i| format!("line {i}\n")).collect();
        let last_a = size - 1 - gap;
        let a_start = a % (last_a + 1);
        let a_end = (a_start + a_len).min(last_a);
        let b_start = a_end + gap;
        let b_end = (b_start + b_len).min(size);
        // An empty range with nothing put in would change nothing.
        let buffer_new = if a_start == a_end { buffer_new.max(1) } else { buffer_new };
        let disk_new = if b_start == b_end { disk_new.max(1) } else { disk_new };
        let ours: Vec<String> = (0..buffer_new).map(|i| format!("ours {i}\n")).collect();
        let theirs: Vec<String> = (0..disk_new).map(|i| format!("theirs {i}\n")).collect();
        let replaced = |range: std::ops::Range<usize>, with: &[String]| {
            let mut lines = base.clone();
            lines.splice(range, with.iter().cloned());
            lines.concat()
        };
        let buffer = replaced(a_start..a_end, &ours);
        let disk = replaced(b_start..b_end, &theirs);
        let mut both = base.clone();
        both.splice(b_start..b_end, theirs);
        both.splice(a_start..a_end, ours);
        let expected = both.concat();
        let base = base.concat();
        prop_assert_eq!(merge::reconcile(&base, &buffer, &disk), Reconciliation::Merged(expected.clone()));
        prop_assert_eq!(merge::reconcile(&base, &disk, &buffer), Reconciliation::Merged(expected));
    }

    #[test]
    fn a_conflict_marks_each_hunk_and_its_choices_take_one_side(
        base in proptest::collection::vec(0u8..4, 0..12),
        buffer in proptest::collection::vec(0u8..4, 0..12),
        disk in proptest::collection::vec(0u8..4, 0..12),
    ) {
        let lines = |ids: &[u8]| ids.iter().map(|id| format!("{id}\n")).collect::<String>();
        let (base, buffer, disk) = (lines(&base), lines(&buffer), lines(&disk));
        match merge::reconcile(&base, &buffer, &disk) {
            Reconciliation::Reload => prop_assert_eq!(&buffer, &base),
            Reconciliation::KeepBuffer => prop_assert!(disk == buffer || disk == base),
            Reconciliation::Merged(text) => {
                prop_assert!(merge::conflict_markers(&text).is_empty());
                if let Reconciliation::Merged(other) = merge::reconcile(&base, &disk, &buffer) {
                    prop_assert_eq!(text, other);
                }
            }
            Reconciliation::Conflict(conflict) => {
                let hunks = conflict.hunks().len();
                prop_assert!(hunks > 0);
                let marked = conflict.with_markers("ours", "theirs");
                prop_assert_eq!(merge::conflict_markers(&marked).len(), hunks);
                let ours = conflict.resolve(&vec![merge::Choice::Buffer; hunks]);
                let theirs = conflict.resolve(&vec![merge::Choice::Disk; hunks]);
                for hunk in conflict.hunks() {
                    prop_assert!(ours.contains(&hunk.buffer) && theirs.contains(&hunk.disk));
                }
            }
        }
    }
}
