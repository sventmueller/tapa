//! The self-test's measure of switching (O25, O29): `switchtime:<tab|workspace>.<count>` shows the next tab or
//! workspace of the window in front `count` times, 150 ms apart, and prints how long each switch held the main thread
//! until the window in front had drawn, and how late the main queue's next turn came after it; `selecttab:<index>`
//! brings the tab at that index of the workspace in front forward as its sidebar row does, which a self-test running
//! behind another app cannot do through the tab bar.

use std::time::{Duration, Instant};

use term_config::Action;

use crate::state::app;

/// How long the steps wait between two switches.
const APART: Duration = Duration::from_millis(150);

/// Runs the selection or the full series of switches before starting the next step.
pub(super) fn run(step: &str, done: impl FnOnce() + Send + 'static) {
    if let Some(index) = step.strip_prefix("selecttab:") {
        select_tab(index.parse().unwrap_or(0));
        return done();
    }
    let Some(spec) = step.strip_prefix("switchtime:") else {
        return done();
    };
    let (what, count) = spec.split_once('.').unwrap_or((spec, "8"));
    let count = count.parse().unwrap_or(8);
    let action = match what {
        "tab" => Action::NextTab,
        "workspace" => Action::NextWorkspace,
        other => {
            eprintln!("selftest: switchtime:{other} is not tab or workspace");
            return done();
        }
    };
    switch(action, what.to_string(), count, Vec::new(), done);
}

/// One switch, then the next after `APART`; prints the samples, each the held and the late milliseconds, at the end.
fn switch(action: Action, what: String, left: u32, mut samples: Vec<(f64, f64)>, done: impl FnOnce() + Send + 'static) {
    if left == 0 {
        report(&what, &samples);
        return done();
    }
    let start = Instant::now();
    crate::menu::perform(action);
    if let Some(window) = app().front().and_then(|window| window.front_ns_window()) {
        window.displayIfNeeded();
    }
    let held = start.elapsed();
    let asked = Instant::now();
    term_base::main_after(Duration::ZERO, move || {
        samples.push((ms(held), ms(asked.elapsed())));
        term_base::main_after(APART, move || switch(action, what, left - 1, samples, done));
    });
}

fn select_tab(index: usize) {
    let Some(window) = app().front() else { return };
    let tab = window
        .selected_workspace()
        .and_then(|workspace| workspace.tabs().get(index).map(|tab| tab.id));
    match tab {
        Some(tab) => window.select(tab),
        None => println!("selecttab {index}: no such tab"),
    }
}

fn ms(span: Duration) -> f64 {
    span.as_secs_f64() * 1000.0
}

fn report(what: &str, samples: &[(f64, f64)]) {
    let mut held: Vec<f64> = samples.iter().map(|sample| sample.0).collect();
    held.sort_by(f64::total_cmp);
    let late = samples.iter().map(|sample| sample.1).fold(0.0, f64::max);
    let median = held.get(held.len() / 2).copied().unwrap_or_default();
    let worst = held.last().copied().unwrap_or_default();
    println!(
        "switchtime {what}: {} switches, held median {median:.1} ms, worst {worst:.1} ms, next turn late at most {late:.1} ms; each {:?}",
        samples.len(),
        samples
            .iter()
            .map(|(held, late)| format!("{held:.1}+{late:.1}"))
            .collect::<Vec<_>>()
    );
}
