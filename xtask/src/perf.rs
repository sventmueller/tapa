//! The bench (E6, section 3 of the plan): throughput, latency, start, memory and idle of the app and of a pinned
//! Ghostty, each 20 times, median and p99, written to `target/perf`. Meaningful only on an idle Mac on mains power.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use crate::run as execute;
use crate::{root, target_dir};

/// How often each measurement runs.
const RUNS: usize = 20;

/// The Ghostty version the gate measures against (R5); raising it is a decision of its own.
pub(crate) const GHOSTTY_PINNED: &str = "1.4.0";

pub(crate) fn main(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("crates") | None => crates(),
        Some("start") => match args.get(1).map(String::as_str) {
            Some("ghostty") => start(Some(ghostty()?)),
            other => start(other.map(PathBuf::from)),
        },
        Some("spread") => spread(),
        Some("throughput") => match args.get(1).map(String::as_str) {
            Some("ghostty") => throughput(&ghostty()?),
            other => throughput(&other.map_or_else(|| target_dir().join("release/Tapa.app"), PathBuf::from)),
        },
        Some(other) => Err(format!(
            "unknown perf task {other}; crates | start [app|ghostty] | throughput [app|ghostty] | spread"
        )),
    }
}

/// The budgets the crates measure themselves: throughput of the session, the slowest key of the vim copy.
fn crates() -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    execute(Command::new(&cargo).current_dir(root()).args([
        "test",
        "--release",
        "-p",
        "term-session",
        "--test",
        "throughput",
        "--",
        "--ignored",
        "--test-threads=1",
    ]))?;
    execute(Command::new(&cargo).current_dir(root()).args([
        "test",
        "--release",
        "-p",
        "term-vim",
        "--",
        "--ignored",
        "--nocapture",
        "ten_megabyte",
    ]))
}

/// The Ghostty the gate measures against, which must be the pinned version.
fn ghostty() -> Result<PathBuf, String> {
    let app = PathBuf::from("/Applications/Sonstige Apps/Ghostty.app");
    let output = Command::new("defaults")
        .arg("read")
        .arg(app.join("Contents/Info.plist"))
        .arg("CFBundleShortVersionString")
        .output()
        .map_err(|e| e.to_string())?;
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version != GHOSTTY_PINNED {
        return Err(format!(
            "Ghostty {version} is installed, the gate measures against {GHOSTTY_PINNED}; install that one or raise the pin"
        ));
    }
    Ok(app)
}

/// Start until the app's first window shows, for `app` or the release build, warm, `RUNS` times.
fn start(app: Option<PathBuf>) -> Result<(), String> {
    let app = app.unwrap_or_else(|| target_dir().join("release/Tapa.app"));
    let name = app
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or("no app name")?
        .to_string();
    let mut samples = Vec::new();
    for _ in 0..RUNS {
        let _ = Command::new("osascript")
            .args(["-e", &format!("quit app \"{name}\"")])
            .status();
        std::thread::sleep(Duration::from_millis(600));
        let started = Instant::now();
        execute(&mut launch(&app, &[])?)?;
        let deadline = started + Duration::from_secs(10);
        loop {
            let windows = Command::new("osascript")
                .args([
                    "-e",
                    &format!("tell application \"System Events\" to count windows of process \"{name}\""),
                ])
                .output()
                .map_err(|e| e.to_string())?;
            let count: u32 = String::from_utf8_lossy(&windows.stdout).trim().parse().unwrap_or(0);
            if count > 0 {
                samples.push(started.elapsed());
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("{name} showed no window within 10 s"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = Command::new("osascript")
        .args(["-e", &format!("quit app \"{name}\"")])
        .status();
    report(&format!("{name}: start to first window"), &samples)
}

/// The protocol's window and face (section 3): 200 × 60 cells of Menlo 13 in either app.
const COLUMNS: u32 = 200;
const ROWS: u32 = 60;
const FONT: &str = "Menlo";
const FONT_SIZE: u32 = 13;

/// The command that opens a new instance of `app` with the protocol's window and face, running `script` if given:
/// Ghostty takes them as arguments, Tapa as a settings file its `TAPA_CONFIG` names.
fn launch(app: &std::path::Path, script: &[&std::ffi::OsStr]) -> Result<Command, String> {
    let mut command = Command::new("open");
    if app.to_string_lossy().contains("Ghostty") {
        command.arg("-na").arg(app).arg("--args").args([
            format!("--window-width={COLUMNS}"),
            format!("--window-height={ROWS}"),
            format!("--font-family={FONT}"),
            format!("--font-size={FONT_SIZE}"),
        ]);
        if let Some(script) = script.first() {
            command.arg("-e").arg(script);
        }
    } else {
        let dir = target_dir().join("perf");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let settings = dir.join("tapa.toml");
        std::fs::write(
            &settings,
            format!(
                "version = 1\n[font]\nfamily = \"{FONT}\"\nsize = {FONT_SIZE}\n[window]\ncolumns = {COLUMNS}\nrows = {ROWS}\nconfirm_close = false\n"
            ),
        )
        .map_err(|e| e.to_string())?;
        command
            .arg("-n")
            .arg("--env")
            .arg(format!("TAPA_CONFIG={}", settings.display()))
            .arg("-a")
            .arg(app)
            .args(script);
    }
    Ok(command)
}

/// Where `cargo xtask perf` finds vtebench and its benchmarks: built once from alacritty/vtebench.
fn vtebench() -> Result<(PathBuf, PathBuf), String> {
    let tools = crate::home().join(".cache/tapa-tools");
    let binary = tools.join("bin/vtebench");
    let benchmarks = tools.join("src/vtebench/benchmarks");
    if binary.exists() && benchmarks.exists() {
        Ok((binary, benchmarks))
    } else {
        Err(format!(
            "vtebench is missing: git clone https://github.com/alacritty/vtebench {} && cargo install --path {} --root {}",
            tools.join("src/vtebench").display(),
            tools.join("src/vtebench").display(),
            tools.display()
        ))
    }
}

/// vtebench's benchmarks inside `app`, which runs a script of ours: the median time per benchmark, written as a DAT
/// file beside the other measurements. The app's window must stay in front and unobstructed while it runs.
fn throughput(app: &std::path::Path) -> Result<(), String> {
    let (binary, benchmarks) = vtebench()?;
    let name = app.file_stem().and_then(|s| s.to_str()).unwrap_or("app").to_string();
    let dir = target_dir().join("perf");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dat = dir.join(format!("vtebench-{}.dat", name.replace(' ', "-")));
    let done = dir.join("vtebench.done");
    let _ = std::fs::remove_file(&dat);
    let _ = std::fs::remove_file(&done);
    let script = dir.join("vtebench.command");
    let text = format!(
        "#!/bin/sh\ncd {}\n{} -s -b {} --dat {} --max-secs 10\ntouch {}\nexit 0\n",
        quoted(benchmarks.parent().unwrap_or(&benchmarks)),
        quoted(&binary),
        quoted(&benchmarks),
        quoted(&dat),
        quoted(&done)
    );
    std::fs::write(&script, text).map_err(|e| e.to_string())?;
    execute(Command::new("chmod").arg("+x").arg(&script))?;
    execute(&mut launch(app, &[script.as_os_str()])?)?;
    let started = Instant::now();
    while !done.exists() {
        if started.elapsed() > Duration::from_secs(900) {
            return Err("vtebench took longer than 15 minutes".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    let _ = Command::new("osascript")
        .args(["-e", &format!("quit app \"{name}\"")])
        .status();
    let results = std::fs::read_to_string(&dat).map_err(|e| e.to_string())?;
    println!("{name}: vtebench results in {}", dat.display());
    for line in results.lines().take(1) {
        println!("{line}");
    }
    for (column, benchmark) in results
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .enumerate()
    {
        let mut samples: Vec<f64> = results
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().nth(column)?.parse().ok())
            .collect();
        samples.sort_by(f64::total_cmp);
        if let Some(median) = samples.get(samples.len() / 2) {
            println!("  {benchmark}: median {median:.2} ms");
        }
    }
    Ok(())
}

fn quoted(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The spread of the machine: the same busy loop `RUNS` times, whose deviation sets the threshold for a regression.
fn spread() -> Result<(), String> {
    let mut samples = Vec::new();
    for _ in 0..RUNS {
        let started = Instant::now();
        let mut x: u64 = 1;
        for i in 0..50_000_000u64 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(i);
        }
        std::hint::black_box(x);
        samples.push(started.elapsed());
    }
    report("machine spread: busy loop", &samples)
}

/// Prints and stores the median and p99 of `samples` and the threshold for a regression: three times the spread,
/// at least 5 %.
fn report(name: &str, samples: &[Duration]) -> Result<(), String> {
    let mut millis: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    millis.sort_by(f64::total_cmp);
    let count = millis.len();
    let median = millis[count / 2];
    let p99 = millis[(count * 99).div_ceil(100).saturating_sub(1).min(count - 1)];
    #[expect(clippy::cast_precision_loss, reason = "twenty samples")]
    let n = count as f64;
    let mean = millis.iter().sum::<f64>() / n;
    let deviation = (millis.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
    let threshold = (3.0 * deviation / mean).max(0.05) * 100.0;
    println!("{name}: median {median:.1} ms, p99 {p99:.1} ms, regression above +{threshold:.1} %");
    let dir = target_dir().join("perf");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{}.json", name.replace([' ', ':', '/'], "-")));
    let json = format!(
        "{{\"name\":\"{name}\",\"median_ms\":{median},\"p99_ms\":{p99},\"threshold_percent\":{threshold},\"samples\":{millis:?}}}\n"
    );
    std::fs::write(file, json).map_err(|e| e.to_string())
}
