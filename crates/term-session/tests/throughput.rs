//! E25: `cat` of a large file must not freeze the view. The reader keeps up with the PTY whether or not anyone draws,
//! and however much arrives, the view hears of it once until it draws again.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

#[allow(dead_code, reason = "each test binary uses its own part of the shared helpers")]
mod common;

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use term_base::LockIgnoringPoison;

use common::{command, temporary};
use term_base::{Ending, Processes, Spawner};
use term_session::{Event, Session};

/// Megabytes per run; a debug build of the emulator parses an order of magnitude slower than a release build.
const MEGABYTES: usize = if cfg!(debug_assertions) { 16 } else { 100 };

/// The lowest acceptable rate in MB/s.
const FLOOR: f64 = if cfg!(debug_assertions) { 2.0 } else { 40.0 };

const LAST_LINE: &str = "END-OF-STREAM";

fn write_sample(path: &std::path::Path) -> usize {
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut written = 0;
    let mut n = 0u64;
    while written < MEGABYTES * 1024 * 1024 {
        let line = format!(
            "{n:010} \x1b[32mok\x1b[0m src/module_{}.rs: compiling \x1b[1mcrate\x1b[22m with a fairly long line of text\n",
            n % 97
        );
        file.write_all(line.as_bytes()).unwrap();
        written += line.len();
        n += 1;
    }
    file.write_all(format!("{LAST_LINE}\n").as_bytes()).unwrap();
    written
}

struct Run {
    rate: f64,
    redraws: usize,
    last_line_shown: bool,
}

fn run(draw_at_60_hz: bool) -> Run {
    // The runs would slow each other down.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _turn = ONE_AT_A_TIME.lock_ignoring_poison();
    let dir = temporary("throughput");
    let sample = dir.join("sample.txt");
    let bytes = write_sample(&sample);
    let supervisor: Arc<dyn Spawner> = Arc::new(Processes::new());
    let redraws = Arc::new(AtomicUsize::new(0));
    let (exited, ended) = channel();
    let counter = Arc::clone(&redraws);
    let started = Instant::now();
    let session = Session::spawn(
        &supervisor,
        &common::environment(&dir),
        command("/bin/cat", &[&sample.to_string_lossy()], &dir),
        move |event| match event {
            Event::Redraw => {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            Event::Exited(ending) => {
                let _ = exited.send(ending);
            }
            _ => {}
        },
    )
    .unwrap();
    let session = Arc::new(session);
    let stop = Arc::new(AtomicBool::new(false));
    let view = draw_at_60_hz.then(|| {
        let session = Arc::clone(&session);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                drop(session.snapshot());
                std::thread::sleep(Duration::from_millis(16));
            }
        })
    });
    let ending = ended.recv_timeout(Duration::from_secs(600)).expect("cat ends");
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    if let Some(view) = view {
        view.join().unwrap();
    }
    assert_eq!(ending, Ending::Code(0));
    let last_line_shown = session.visible_text().iter().any(|line| line == LAST_LINE);
    #[expect(clippy::cast_precision_loss, reason = "a rate for a test report")]
    let rate = bytes as f64 / 1024.0 / 1024.0 / elapsed.as_secs_f64();
    let name = if draw_at_60_hz {
        "while drawing at 60 Hz"
    } else {
        "while nobody draws"
    };
    term_base::perf::value(
        &format!("term-session: reading a large cat {name}"),
        "MB/s",
        false,
        rate,
    );
    eprintln!(
        "{MEGABYTES} MB in {elapsed:?}: {rate:.1} MB/s, {} redraws{}",
        redraws.load(Ordering::Relaxed),
        if draw_at_60_hz { " while drawing at 60 Hz" } else { "" }
    );
    Run {
        rate,
        redraws: redraws.load(Ordering::Relaxed),
        last_line_shown,
    }
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn a_large_cat_is_read_at_full_speed_while_nobody_draws_and_asks_for_one_redraw() {
    let run = run(false);
    assert!(run.last_line_shown, "the whole stream reached the screen");
    assert_eq!(
        run.redraws, 1,
        "redraws are not queued up for a view that does not draw"
    );
    assert!(run.rate >= FLOOR, "{:.1} MB/s is below {FLOOR} MB/s", run.rate);
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn a_large_cat_keeps_its_speed_while_the_view_draws_at_60_hz() {
    let run = run(true);
    assert!(run.last_line_shown, "the whole stream reached the screen");
    assert!(run.rate >= FLOOR, "{:.1} MB/s is below {FLOOR} MB/s", run.rate);
}
