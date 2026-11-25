//! The core against the reference cases taken over from Alacritty (E11): every case's screen and cells as
//! `alacritty_terminal` left them, unless `deviations.md` names a deliberate difference and `expected.own` holds the
//! screen with it.

use std::path::Path;

use term_vt::reference::replay;
use term_vt::term::Size;

#[test]
fn every_reference_case() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ref");
    let mut cases: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    let mut failed = Vec::new();
    for case in &cases {
        // `expected.own` holds the screen where deviations.md names a deliberate difference from Alacritty.
        let expected = std::fs::read_to_string(case.join("expected.own"))
            .or_else(|_| std::fs::read_to_string(case.join("expected")))
            .unwrap();
        let recording = std::fs::read(case.join("recording")).unwrap();
        let header: Vec<usize> = expected
            .lines()
            .next()
            .unwrap()
            .split(' ')
            .skip(1)
            .map(|n| n.parse().unwrap())
            .collect();
        let size = Size {
            columns: header[0],
            lines: header[1],
        };
        let got = replay(&recording, size, header[2]);
        if got != expected {
            let first = got
                .lines()
                .zip(expected.lines())
                .find(|(a, b)| a != b)
                .map(|(a, b)| format!("got {a:?}, want {b:?}"))
                .unwrap_or_else(|| format!("{} lines, want {}", got.lines().count(), expected.lines().count()));
            failed.push(format!("{}: {first}", case.file_name().unwrap().to_string_lossy()));
        }
    }
    let passed = cases.len() - failed.len();
    assert!(
        failed.is_empty(),
        "{passed} of {} pass\n{}",
        cases.len(),
        failed.join("\n")
    );
}
