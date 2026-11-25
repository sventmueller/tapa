//! The replacement Vim hands the editor turns the text before into the text after, checked over generated edits
//! of ropes that share their unchanged pieces and of ropes that share none (E96).

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use ropey::Rope;

use crate::diff::difference;

fn config() -> Config {
    Config {
        cases: 256,
        rng_seed: RngSeed::Fixed(0x0071_ed17),
        ..Config::default()
    }
}

/// Text from pieces of one to four bytes and line ends, long enough to span many of a rope's leaves.
fn text(most: usize) -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("a"),
        Just("xyz"),
        Just("ä"),
        Just("😀"),
        Just("e\u{301}"),
        Just("\n")
    ];
    proptest::collection::vec(piece, 0..most).prop_map(|pieces| pieces.concat())
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn the_replacement_turns_the_text_before_into_the_text_after(
        start in text(1500),
        edits in proptest::collection::vec((any::<usize>(), any::<usize>(), text(20)), 1..5),
        fresh in any::<bool>(),
    ) {
        let before = Rope::from_str(&start);
        let mut after = before.clone();
        for (a, b, new) in &edits {
            let len = after.len_chars() + 1;
            let (from, to) = (a % len, b % len);
            let (from, to) = (from.min(to), from.max(to));
            after.remove(from..to);
            after.insert(from, new);
        }
        if fresh {
            after = Rope::from_str(&after.to_string());
        }
        let Some(replacement) = difference(&before, &after) else {
            prop_assert_eq!(before.to_string(), after.to_string());
            return Ok(());
        };
        let mut applied = before.clone();
        let at = |(line, column): (usize, usize)| before.line_to_char(line) + column;
        let (from, to) = (at(replacement.start), at(replacement.end));
        prop_assert!(from <= to && to <= before.len_chars(), "{:?}", replacement);
        applied.remove(from..to);
        applied.insert(from, &replacement.text);
        prop_assert_eq!(applied.to_string(), after.to_string());
    }
}
