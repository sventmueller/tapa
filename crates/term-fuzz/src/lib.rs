//! Test-only helpers of the differential fuzzing (E11.3): the generator of output, and libghostty-vt as the core to
//! compare with where it is built (`ghostty`).

pub mod ghostty;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};

/// One piece of output for comparing with libghostty-vt: every sequence, each CSI with as many parameters as it takes
/// and none of 0, as Ghostty ignores one with more where xterm reads the first, and takes 0 literally where xterm reads
/// the default.
pub fn piece() -> impl Strategy<Value = Vec<u8>> {
    let text = prop_oneof![
        4 => "[a-zA-Z0-9 .,:;!?-]{1,12}".prop_map(String::into_bytes),
        1 => Just("漢字".as_bytes().to_vec()),
        1 => Just("e\u{301}".as_bytes().to_vec()),
        1 => Just("😀".as_bytes().to_vec()),
    ];
    let control = prop::sample::select(vec!["\r", "\n", "\t", "\x08"]).prop_map(|c: &str| c.as_bytes().to_vec());
    let actions: &[u8] = b"@ABCDEFGHIJKLMPSTXZdfr`ab";
    let csi = (0u16..30, 0u16..30, prop::sample::select(actions.to_vec())).prop_map(|(a, b, action)| {
        let action = char::from(action);
        if "fHr".contains(action) {
            format!("\x1b[{a};{b}{action}").into_bytes()
        } else {
            format!("\x1b[{}{action}", a.max(1)).into_bytes()
        }
    });
    let erase = (0u16..4, prop::sample::select(b"JK".to_vec()))
        .prop_map(|(mode, action)| format!("\x1b[{mode}{}", char::from(action)).into_bytes());
    let sgr = prop::collection::vec(
        prop_oneof![
            (0u16..10).prop_map(|n| n.to_string()),
            (30u16..38).prop_map(|n| n.to_string()),
            (40u16..48).prop_map(|n| n.to_string()),
            (90u16..98).prop_map(|n| n.to_string()),
            (0u16..256).prop_map(|n| format!("38;5;{n}")),
            (0u16..256, 0u16..256, 0u16..256).prop_map(|(r, g, b)| format!("48;2;{r};{g};{b}")),
            Just("4:3".to_string()),
            Just("22".to_string()),
            Just("27".to_string()),
        ],
        0..4,
    )
    .prop_map(|parts| format!("\x1b[{}m", parts.join(";")).into_bytes());
    let modes = prop::sample::select(vec![
        "\x1b7",
        "\x1b8",
        "\x1bE",
        "\x1bH",
        "\x1b[3g",
        "\x1b[g",
        "\x1b(0",
        "\x1b(B",
        "\x1b[s",
        "\x1b[u",
        "\x1b[?7l",
        "\x1b[?1049h",
        "\x1b[?6h",
        "\x1b[?6l",
        "\x1b[?7h",
        "\x1b[4h",
        "\x1b[4l",
        "\x1b[?1049l",
        "\x1bD",
        "\x1bM",
        "\x1b#8",
    ])
    .prop_map(|s: &str| s.as_bytes().to_vec());
    prop_oneof![4 => text, 2 => control, 3 => csi, 1 => erase, 2 => sgr, 2 => modes]
}

/// Output of up to 60 pieces for comparing with Ghostty.
pub fn ghostty_output() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(piece(), 1..60).prop_map(|pieces| pieces.concat())
}

/// 2000 cases with a fixed seed; `TERM_FUZZ_CASES` and `TERM_FUZZ_SEED` widen the search, as `make fuzz` does.
pub fn config() -> Config {
    let number = |name: &str, default: u64| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    Config {
        cases: u32::try_from(number("TERM_FUZZ_CASES", 2000)).unwrap_or(2000),
        rng_seed: RngSeed::Fixed(number("TERM_FUZZ_SEED", 0x7e57_d1ff)),
        ..Config::default()
    }
}

/// The final byte and parameters of each CSI sequence in `output`.
pub fn csi(output: &[u8]) -> Vec<(u8, Vec<u16>)> {
    let mut found = Vec::new();
    let mut rest = output;
    while let Some(at) = rest.windows(2).position(|w| w == b"\x1b[") {
        rest = &rest[at + 2..];
        let end = rest.iter().position(|b| (0x40..0x7f).contains(b)).unwrap_or(rest.len());
        if let Some(&last) = rest.get(end) {
            let params = String::from_utf8_lossy(&rest[..end])
                .split(';')
                .filter_map(|p| p.split(':').next().and_then(|p| p.parse().ok()))
                .collect();
            found.push((last, params));
        }
        rest = &rest[end.min(rest.len())..];
    }
    found
}

/// Whether `output` may insert, delete or erase characters next to a wide one, splitting it.
pub fn may_split_wide(output: &[u8]) -> bool {
    let wide = ["漢字", "😀"]
        .iter()
        .any(|w| output.windows(w.len()).any(|window| window == w.as_bytes()));
    let insert_mode = output.windows(4).any(|window| window == b"\x1b[4h");
    wide && (insert_mode
        || csi(output)
            .iter()
            .any(|(last, _)| matches!(last, b'@' | b'P' | b'X' | b'J' | b'K')))
}
