//! esctest (E11 point 4, E4's gate) run inside the app: the maintained fork is cloned into the tool
//! cache, never into the repository; `crates/term-vt/tests/ref/esctest.md` records the outcome and the flags.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::{home, run, selftest, target_dir};

const REPOSITORY: &str = "https://github.com/ThomasDickey/esctest2";

/// What the run claims to be: a recent xterm, whose checksums count empty cells as blanks (patch 334) and whose
/// reverse wrap is mode 1045 (patch 383); window operations and 8-bit controls stay off.
const FLAGS: &[&str] = &[
    "--expected-terminal=xterm",
    "--max-vt-level=5",
    "--xterm-checksum=334",
    "--xterm-reverse-wrap=383",
    "--timeout=1",
    "--no-print-logs",
    "--v=3",
];

/// How one test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    KnownBug,
    Skipped,
}

/// `cargo xtask esctest [--include <regex>]`.
pub(crate) fn main(args: &[String]) -> Result<(), String> {
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .map(String::as_str)
    };
    let include = value("--include").unwrap_or(".*");
    if include.contains([',', '\'']) {
        return Err("the --include pattern may hold neither a comma nor a single quote".into());
    }
    let checkout = checkout()?;
    let commit = Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    println!("esctest {commit}, flags {}", FLAGS.join(" "));
    let outcomes = run_in_app(&checkout, include)?;
    let count = |which: Outcome| outcomes.values().filter(|&&o| o == which).count();
    println!(
        "{} passed, {} failed, {} known bugs, {} skipped",
        count(Outcome::Passed),
        count(Outcome::Failed),
        count(Outcome::KnownBug),
        count(Outcome::Skipped)
    );
    for (name, _) in outcomes.iter().filter(|(_, o)| **o == Outcome::Failed) {
        println!("  failed {name}");
    }
    Ok(())
}

/// The esctest folder of the checkout in the tool cache, cloned on first use.
fn checkout() -> Result<PathBuf, String> {
    let source = home().join(".cache/tapa-tools/src/esctest2");
    if !source.join("esctest/esctest.py").exists() {
        std::fs::create_dir_all(source.parent().unwrap_or(&source)).map_err(|e| e.to_string())?;
        run(Command::new("git").args(["clone", "-q", REPOSITORY]).arg(&source))?;
    }
    Ok(source)
}

/// Runs the tests matching `include` in the app and reads their outcomes from esctest's log.
fn run_in_app(checkout: &Path, include: &str) -> Result<BTreeMap<String, Outcome>, String> {
    let dir = target_dir().join("esctest");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let config = dir.join("settings.toml");
    let settings = "version = 1\n[window]\ncolumns = 80\nrows = 25\nconfirm_close = false\n\
         [terminal]\nshell_integration = false\n[input]\nfield = false\n";
    std::fs::write(&config, settings).map_err(|e| e.to_string())?;
    let log = dir.join("esctest.log");
    let done = dir.join("esctest.done");
    for stale in [&log, &done] {
        let _ = std::fs::remove_file(stale);
    }
    let script = checkout.join("esctest/esctest.py");
    for path in [&script, &log, &done] {
        if path.to_string_lossy().contains([',', ' ', '\'']) {
            return Err(format!(
                "{}: the self-test cannot type a path with commas or spaces",
                path.display()
            ));
        }
    }
    let command = format!(
        "python3 {} {} --logfile={} --include='{include}'; touch {}",
        script.display(),
        FLAGS.join(" "),
        log.display(),
        done.display()
    );
    let steps = format!("wait:1000,type:clear; {command}\\n,until:{}", done.display());
    selftest(&steps, false, Some(&config), Duration::from_secs(3600))?;
    let text = std::fs::read_to_string(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    Ok(outcomes(&text))
}

/// Each test's outcome by name, from the lines esctest logs at level 2.
fn outcomes(log: &str) -> BTreeMap<String, Outcome> {
    let mut outcomes = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in log.lines() {
        if let Some(name) = line.strip_prefix("Run test: ") {
            current = Some(name.to_string());
            continue;
        }
        let outcome = if line == "Passed." {
            Outcome::Passed
        } else if line.starts_with("*** TEST ") && line.ends_with(" FAILED:") {
            Outcome::Failed
        } else if line.starts_with("Fails as expected") {
            Outcome::KnownBug
        } else if line.starts_with("Skipped because") {
            Outcome::Skipped
        } else {
            continue;
        };
        if let Some(name) = current.take() {
            outcomes.insert(name, outcome);
        }
    }
    outcomes
}

#[cfg(test)]
mod tests {
    use super::{Outcome, outcomes};

    #[test]
    fn the_log_gives_each_test_its_outcome() {
        let log = "Run test: A.test_one\nPassed.\n\nRun test: A.test_two\n*** TEST A.test_two FAILED:\nTraceback\n\n\
                   Run test: A.test_three\nSkipped because terminal lacks requisite capability: 4\n";
        let found = outcomes(log);
        assert_eq!(found["A.test_one"], Outcome::Passed);
        assert_eq!(found["A.test_two"], Outcome::Failed);
        assert_eq!(found["A.test_three"], Outcome::Skipped);
    }
}
