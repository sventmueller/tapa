//! The instant stage's budget (PLAN section 3, E8): p99 under 2 ms per key for the history's suggestion and the
//! spec's completion together, with a full history in memory. Debug builds only report the numbers; the budget is
//! checked with `cargo test --release -p term-suggest --test latency`.

use std::time::{Duration, Instant};

use term_suggest::{Context, Entry, History, complete, complete_with_sources};

const LINES: &[&str] = &[
    "git commit --amend --no-edit",
    "git checkout -b feature/x",
    "cargo test -p term-suggest --release",
    "docker compose up -d",
    "kubectl get pods -n default",
    "bun run dev",
    "ls -la src/",
    "sudo git status",
];

fn history() -> History {
    let mut history = History::in_memory();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for (index, at) in (0..50_000u64).enumerate() {
        let line = format!("{} {}", LINES[index % LINES.len()], at % 700);
        history.record(Entry {
            line,
            folder: format!("/p/{}", at % 40).into(),
            branch: Some("main".into()),
            previous: None,
            exit_code: Some(0),
            started: now - at * 60,
        });
    }
    history
}

#[test]
fn every_key_of_the_instant_stage_stays_within_two_milliseconds() {
    let dir = term_base::testing::scratch("suggest-latency");
    std::fs::create_dir_all(dir.join("src/bin")).unwrap();
    let history = history();
    let context = Context {
        folder: "/p/3".into(),
        branch: Some("main".into()),
        ..Context::default()
    };
    for line in LINES {
        // The field warms a spec once its command is typed; the instant stage never parses one.
        complete_with_sources(line, &dir, &dir, Instant::now());
    }
    let mut times: Vec<Duration> = Vec::new();
    for line in LINES {
        for end in (1..=line.len()).filter(|end| line.is_char_boundary(*end)) {
            let typed = &line[..end];
            let started = Instant::now();
            let _ = history.suggest(typed, &context);
            let _ = complete(typed, &dir, &dir, false);
            times.push(started.elapsed());
        }
    }
    times.sort();
    let p99 = times[times.len() * 99 / 100];
    let worst = times[times.len() - 1];
    println!("instant stage over {} keys: p99 {p99:?}, worst {worst:?}", times.len());
    if !cfg!(debug_assertions) {
        assert!(p99 < Duration::from_millis(2), "p99 {p99:?}");
    }
}
