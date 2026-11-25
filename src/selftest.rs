//! The self-test (F208): with `TAPA_SELFTEST` set to steps separated by commas, the app runs them one after the
//! other once its first window shows, has the runner take a picture of the front window after each `shot:<name>`
//! into `TAPA_SELFTEST_DIR` and quits after the last. A step is an action of the key bindings by its name, `type:<text>`
//! for text typed into the session, `text:<name>` or `lines:<name>` (with the empty lines, also kept as `<name>.txt`
//! in that folder) for the screen's text,
//! `wait:<ms>`, `until:<path>` (waits until the file exists), `settings:<pane>` (opens the settings window on the pane
//! `window`, `font`, `theme`, `cursor`, `terminal`, `input`, `agents`, `profiles` or `keys`), `setting:<path>=<TOML value>`
//! (writes the file as a control of that window does) or `shot:<name>`, of the settings window while it is in front.
//! `menu:<name>` prints the menu bar as AppKit validated it, and `alert:quit` shows the alert before quitting, takes
//! the picture `alert-quit` of it and cancels it. The agent steps are in `selftest/agent.rs` and, for installing
//! agents, `selftest/agent_install.rs`, the browser
//! steps in `selftest/browser.rs`, those of files, panels and git in `selftest/files.rs`, and `modal:` for the steps
//! that ask in an alert or a save panel in `selftest/modal.rs`; `axtree:<name>` dumps the accessibility tree
//! (`selftest/accessibility.rs`).

use std::path::PathBuf;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use term_config::Action;

use crate::state::app;

mod accessibility;
mod agent;
mod agent_install;
mod browser;
mod dictation;
mod files;
mod looks;
pub(crate) mod menus;
mod modal;
mod mouse;
mod preview;
pub(crate) mod stand_in;
mod steps;
mod switching;
mod wheel;
mod workspace;

/// How long the app waits between steps, for shells to answer and frames to draw.
const PAUSE: Duration = Duration::from_millis(400);

/// Starts the steps of `TAPA_SELFTEST`, if it is set.
pub(crate) fn start() {
    let Ok(steps) = std::env::var("TAPA_SELFTEST") else {
        return;
    };
    term_base::unattended::set();
    agent::prepare();
    agent_install::prepare();
    term_ui_git::selftest::isolate_git_config();
    let steps: Vec<String> = steps
        .split(',')
        .map(|step| step.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    term_base::main_after(Duration::from_millis(1200), move || next(steps, 0));
}

fn next(steps: Vec<String>, at: usize) {
    let Some(step) = steps.get(at).cloned() else {
        let mtm = MainThreadMarker::new().expect("the self-test runs on the main thread");
        crate::quit::end_all();
        NSApplication::sharedApplication(mtm).terminate(None);
        return;
    };
    let mut pause = PAUSE;
    if let Some(ms) = step.strip_prefix("wait:") {
        pause = Duration::from_millis(ms.parse().unwrap_or(500));
    } else if let Some(name) = step.strip_prefix("shot:") {
        shot(name);
    } else if let Some((name, all)) = step
        .strip_prefix("text:")
        .map(|name| (name, false))
        .or_else(|| step.strip_prefix("lines:").map(|name| (name, true)))
    {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            let lines = view
                .with_session(term_session::Session::visible_text)
                .unwrap_or_default();
            if all && let Some(dir) = std::env::var_os("TAPA_SELFTEST_DIR") {
                let _ = std::fs::write(PathBuf::from(dir).join(format!("{name}.txt")), lines.join("\n"));
            }
            let lines: Vec<&String> = lines.iter().filter(|line| all || !line.is_empty()).collect();
            println!(
                "text {name} (field {}): {lines:?}",
                if view.field_shown() { "shown" } else { "away" }
            );
        }
    } else if let Some(ms) = step.strip_prefix("stall:") {
        let span = Duration::from_millis(ms.parse().unwrap_or(5000));
        return stall(
            steps,
            at,
            std::time::Instant::now() + span,
            Duration::ZERO,
            std::time::Instant::now(),
        );
    } else if let Some(path) = step.strip_prefix("until:") {
        return until(steps, at, PathBuf::from(path));
    } else if let Some(count) = step.strip_prefix("latency:") {
        return latency(steps, at, count.parse().unwrap_or(100), 0);
    } else if let Some(text) = step.strip_prefix("fieldtype:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            view.type_into_field(text);
            let (text, hint) = view.field_contents();
            println!("field {text:?} hint {hint:?} chips {:?}", view.field_context());
        }
    } else if let Some(text) = step.strip_prefix("field:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view())
            && !view.submit_command(text)
        {
            println!("selftest: the input field is not shown");
        }
    } else if step == "settingsend:" {
        crate::settings::scroll_to_end();
    } else if let Some(pane) = step.strip_prefix("settings:") {
        if !crate::settings::show_pane(pane) {
            eprintln!("selftest: no settings pane {pane}");
        }
    } else if let Some(name) = step.strip_prefix("menu:") {
        menus::print(name);
    } else if let Some(name) = step.strip_prefix("alert:") {
        menus::alert(name);
        pause = menus::ALERT_TIME;
    } else if let Some(name) = step.strip_prefix("tabmenu:") {
        menus::tab_menu(name);
    } else if let Some(name) = step.strip_prefix("closebutton:") {
        menus::close_button(name);
    } else if step == "help:" {
        menus::help();
    } else if step == "about:" {
        menus::about();
    } else if step == "escape:" {
        menus::escape();
        pause = Duration::from_millis(600);
    } else if let Some(assignment) = step.strip_prefix("setting:") {
        setting(assignment);
    } else if let Some(text) = step.strip_prefix("search:") {
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            view.search_for(text);
        }
    } else if let Some(text) = step.strip_prefix("type:") {
        let text = text.replace("\\n", "\n");
        if let Some(view) = app().front().and_then(|window| window.focused_view()) {
            view.with_session(|session| session.write(text.into_bytes()));
        }
    } else if modal::run(&step) {
        pause = modal::TIME;
    } else if let Some(count) = switching::run(&step) {
        pause = switching::time(count);
    } else if perform(&step) {
        if step.starts_with("worktree:") {
            pause = files::WORKTREE_TIME;
        }
    } else {
        eprintln!("selftest: unknown step {step}");
    }
    term_base::main_after(pause, move || next(steps, at + 1));
}

/// Runs `step` if it is one of the modules' steps or a key binding's action; whether it was.
fn perform(step: &str) -> bool {
    if steps::run(step)
        || mouse::run(step)
        || agent::run(step)
        || agent_install::run(step)
        || browser::run(step)
        || files::run(step)
        || preview::run(step)
        || dictation::run(step)
        || modal::other(step)
        || looks::run(step)
        || wheel::run(step)
        || accessibility::run(step)
    {
        return true;
    }
    let action = Action::ALL.iter().find(|action| action.name() == step);
    if let Some(action) = action {
        crate::menu::perform(*action);
    }
    action.is_some()
}

/// Writes `path=value`, such as `font.size=15`, into the settings file the way a control of the settings window
/// does, the value in TOML (`theme.dark="Dracula"`); prints the warnings the file then has.
fn setting(assignment: &str) {
    let Some((path, value)) = assignment.split_once('=') else {
        eprintln!("selftest: setting needs path=value, not {assignment}");
        return;
    };
    let value = match value.trim().parse::<toml_edit::Value>() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("selftest: {value} is no TOML value: {error}");
            return;
        }
    };
    let path: Vec<&str> = path.trim().split('.').collect();
    let written = crate::settings::apply(|doc| {
        doc.set(&path, value);
        Ok(true)
    });
    if let Err(error) = written {
        println!("setting {assignment}: {error}");
    }
    let warnings: Vec<String> = app()
        .warnings
        .borrow()
        .iter()
        .map(|warning| warning.0.clone())
        .collect();
    println!("setting {assignment}: warnings {warnings:?}");
}

/// Goes on once `path` exists, for a program in the session that marks its end with a file.
fn until(steps: Vec<String>, at: usize, path: PathBuf) {
    if path.exists() {
        return next(steps, at + 1);
    }
    term_base::main_after(Duration::from_millis(200), move || until(steps, at, path));
}

/// Watches the main thread until `until`: a hop every 5 ms, each late by how long the thread was blocked; prints the
/// longest block, which the plan's section 3 caps at 50 ms while a gigabyte scrolls by.
fn stall(steps: Vec<String>, at: usize, until: std::time::Instant, worst: Duration, asked: std::time::Instant) {
    let late = asked.elapsed().saturating_sub(Duration::from_millis(5));
    let worst = worst.max(late);
    if std::time::Instant::now() >= until {
        println!(
            "stall: longest block of the main thread {:.1} ms",
            worst.as_secs_f64() * 1000.0
        );
        return next(steps, at + 1);
    }
    let asked = std::time::Instant::now();
    term_base::main_after(Duration::from_millis(5), move || stall(steps, at, until, worst, asked));
}

/// Types `left` more characters into the session in front, one per 30 ms as the bench of the plan's section 3 does,
/// each counted from its write to the frame that shows it; then prints the median and p99 and goes on.
fn latency(steps: Vec<String>, at: usize, left: usize, typed: usize) {
    let view = app().front().and_then(|window| window.focused_view());
    let Some(view) = view else {
        return next(steps, at + 1);
    };
    let id = view.id();
    if left == 0 {
        term_base::main_after(Duration::from_millis(200), move || {
            let Some(view) = term_ui::find(id) else {
                return next(steps, at + 1);
            };
            let mut samples = view.take_latencies();
            samples.sort_unstable();
            if samples.is_empty() {
                println!("latency: no samples");
            } else {
                let median = samples[samples.len() / 2];
                let p99 = samples[(samples.len() * 99).div_ceil(100).saturating_sub(1)];
                println!(
                    "latency: {} samples of {typed}, median {:.2} ms, p99 {:.2} ms",
                    samples.len(),
                    f64::from(median) / 1000.0,
                    f64::from(p99) / 1000.0
                );
            }
            view.with_session(|session| session.write(b"\x15".to_vec()));
            term_base::main_after(PAUSE, move || next(steps, at + 1));
        });
        return;
    }
    view.mark_key();
    view.with_session(|session| session.write(b"x".to_vec()));
    term_base::main_after(Duration::from_millis(30), move || {
        latency(steps, at, left - 1, typed + 1)
    });
}

/// Has the runner take a picture of the front window as it is on screen, Metal layers and all: it reads the line
/// `shot <window number> <path>` from standard output and runs `screencapture`, which may record the screen.
fn shot(name: &str) {
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(|| PathBuf::from("target/selftest"), PathBuf::from);
    let terminal = app().front().and_then(|window| window.front_ns_window());
    let window = crate::settings::ns_window()
        .filter(|settings| in_front_of(settings, terminal.as_deref()))
        .or(terminal);
    let Some(window) = window else {
        eprintln!("selftest: no window for {name}");
        return;
    };
    let path = dir.join(format!("{name}.png"));
    println!("shot {} {}", window.windowNumber(), path.display());
}

/// Whether `window` is ordered in front of `other`, or there is no other.
fn in_front_of(window: &objc2_app_kit::NSWindow, other: Option<&objc2_app_kit::NSWindow>) -> bool {
    let Some(other) = other else { return true };
    let mtm = MainThreadMarker::new().expect("the self-test runs on the main thread");
    let order = NSApplication::sharedApplication(mtm).orderedWindows();
    let place = |wanted: &objc2_app_kit::NSWindow| order.iter().position(|own| std::ptr::eq(&*own, wanted));
    match (place(window), place(other)) {
        (Some(window), Some(other)) => window < other,
        (found, _) => found.is_some(),
    }
}
