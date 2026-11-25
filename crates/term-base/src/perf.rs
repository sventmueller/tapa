//! Named measurements of the timing and memory budgets, behind the feature `testing`: each is written as one JSON file
//! to `TERM_PERF_DIR`, or under a cargo test to the workspace's `target/perf`, where `cargo xtask perf-compare` holds
//! them against `perf/baseline.json` (docs/development.md, "Timing budgets").

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The variable that names the folder measurements go to; the self-test runner sets it for the app.
pub const DIR_VARIABLE: &str = "TERM_PERF_DIR";

/// One measurement: the median of its samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    /// What was measured, the key in the baseline.
    pub name: String,
    /// The median of `samples`.
    pub value: f64,
    /// The unit of `value`, such as `ms`, `MB` or `MB/s`.
    pub unit: String,
    /// Whether a smaller value is the better one, as for a time; a rate is the other way round.
    pub lower_is_better: bool,
    /// The samples of a measurement taken more than once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<f64>,
}

impl Measurement {
    /// The measurement `name` of `samples` in `unit`, its value their median.
    pub fn of(name: &str, unit: &str, lower_is_better: bool, samples: &[f64]) -> Measurement {
        Measurement {
            name: name.to_owned(),
            value: median(samples),
            unit: unit.to_owned(),
            lower_is_better,
            samples: if samples.len() > 1 {
                samples.to_vec()
            } else {
                Vec::new()
            },
        }
    }
}

/// The middle of `samples`, the mean of the two middle ones for an even count; zero without any.
pub fn median(samples: &[f64]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    match n {
        0 => 0.0,
        _ if n % 2 == 1 => sorted[n / 2],
        _ => f64::midpoint(sorted[n / 2 - 1], sorted[n / 2]),
    }
}

/// Records the median of `samples` as `name` in milliseconds, lower being better.
pub fn durations(name: &str, samples: &[Duration]) {
    let millis: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    record(&Measurement::of(name, "ms", true, &millis));
}

/// Records one `value` in `unit` as `name`.
pub fn value(name: &str, unit: &str, lower_is_better: bool, value: f64) {
    record(&Measurement::of(name, unit, lower_is_better, &[value]));
}

/// Writes `measurement` to the folder of measurements, if there is one; a measurement that cannot be written only
/// says so, as the budget's own assertion still decides the test.
pub fn record(measurement: &Measurement) {
    let Some(dir) = folder() else { return };
    if let Err(error) = write(&dir, measurement) {
        eprintln!("perf: {} not written to {}: {error}", measurement.name, dir.display());
    }
}

/// Writes `measurement` into `dir` as a file of its own, named after it.
pub fn write(dir: &Path, measurement: &Measurement) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    crate::atomic_write::write_json(&dir.join(file_name(&measurement.name)), measurement)
}

/// The file `name` is kept in: its letters and digits, every other run of characters one `-`.
pub fn file_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    format!("{}.json", out.trim_matches('-'))
}

fn folder() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(DIR_VARIABLE) {
        return Some(PathBuf::from(dir));
    }
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);
    let root = manifest.ancestors().find(|dir| dir.join("Cargo.lock").is_file())?;
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |dir| root.join(dir));
    Some(target.join("perf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_is_the_middle_sample_or_the_mean_of_the_two_middle_ones() {
        assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < f64::EPSILON);
        assert!((median(&[4.0, 1.0, 3.0, 2.0]) - 2.5).abs() < f64::EPSILON);
        assert!(median(&[]).abs() < f64::EPSILON);
    }

    #[test]
    fn a_name_becomes_a_file_name_of_letters_digits_and_dashes() {
        assert_eq!(
            file_name("snappy: a folder of 5 000 entries"),
            "snappy-a-folder-of-5-000-entries.json"
        );
        assert_eq!(file_name("term-vim/slowest key"), "term-vim-slowest-key.json");
    }

    #[test]
    fn a_measurement_is_written_and_read_back() {
        let dir = crate::testing::scratch("perf-write");
        let measurement = Measurement::of("keystroke", "ms", true, &[1.0, 3.0, 2.0]);
        write(dir.path(), &measurement).unwrap();
        let read: Measurement =
            serde_json::from_slice(&std::fs::read(dir.path().join("keystroke.json")).unwrap()).unwrap();
        assert_eq!(read, measurement);
        assert!((read.value - 2.0).abs() < f64::EPSILON);
    }
}
