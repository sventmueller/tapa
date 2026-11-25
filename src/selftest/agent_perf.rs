//! Exercises visible and hidden streaming timelines with repeatable replay frames.

use std::fmt::Write;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use term_ui_agent::AgentTab;

use crate::state::app;

const SAMPLES: usize = 40;

pub(super) fn run(entries: usize, done: impl FnOnce() + 'static) {
    let Some(window) = app().front() else { return done() };
    let Some(workspace) = window.selected_workspace() else {
        return done();
    };
    let history = history(entries.min(10_000));
    let tabs: Vec<_> = (0..4)
        .map(|index| {
            let tab = term_ui_agent::open_replay_in(
                &workspace.handle(),
                workspace.base.clone(),
                &format!("Replay {}", index + 1),
            );
            tab.feed(&history);
            begin(&tab);
            tab
        })
        .collect();
    let Some((visible, hidden)) = tabs.split_last() else {
        return done();
    };
    visible.select();
    visible.timeline().scroll_to_end();
    visible.timeline().settle();
    let before = hidden.iter().map(|tab| tab.timeline().built()).collect();
    after_frame(move || sample(tabs, before, Vec::new(), done));
}

fn sample(tabs: Vec<Rc<AgentTab>>, before: Vec<usize>, mut times: Vec<f64>, done: impl FnOnce() + 'static) {
    let Some((visible, hidden)) = tabs.split_last() else {
        return done();
    };
    let delta = frame(json!({
        "type":"content_block_delta", "index":0,
        "delta":{"type":"text_delta", "text":"Another **word** in the answer.\n\n"}
    }));
    let start = Instant::now();
    for tab in &tabs {
        tab.feed(&delta);
    }
    visible.timeline().settle();
    times.push(start.elapsed().as_secs_f64() * 1000.0);
    if times.len() < SAMPLES {
        return after_frame(move || sample(tabs, before, times, done));
    }
    let extra: usize = hidden
        .iter()
        .zip(before)
        .map(|(tab, before)| tab.timeline().built().saturating_sub(before))
        .sum();
    times.sort_by(f64::total_cmp);
    println!(
        "agentperf: {} rows, {SAMPLES} batches, median {:.2} ms, p95 {:.2} ms, max {:.2} ms, hidden cards built {extra}",
        visible.timeline().count(),
        times[SAMPLES / 2],
        times[SAMPLES * 95 / 100 - 1],
        times[SAMPLES - 1],
    );
    if let Some(tab) = hidden.first() {
        tab.select();
        let tab = tab.clone();
        after_frame(move || {
            tab.timeline().settle();
            println!(
                "agentperf: returning to hidden tab, {} rows, follows end {}",
                tab.timeline().count(),
                tab.timeline().is_at_bottom()
            );
            done();
        });
    } else {
        done();
    }
}

fn history(entries: usize) -> String {
    let mut lines = String::new();
    for n in 0..entries / 2 {
        let _ = writeln!(
            lines,
            ">> {}",
            json!({"type":"user", "uuid":format!("u{n}"), "message":{"role":"user","content":[{"type":"text","text":format!("Question {n}")}]}})
        );
        let _ = writeln!(
            lines,
            "{}",
            json!({"type":"assistant", "uuid":format!("a{n}"), "message":{"id":format!("m{n}"),"role":"assistant","content":[{"type":"text","text":"A completed **answer**."}]}})
        );
    }
    lines
}

fn frame(event: Value) -> String {
    json!({"type":"stream_event", "event":event, "parent_tool_use_id":null}).to_string()
}

fn begin(tab: &AgentTab) {
    tab.feed(&frame(
        json!({"type":"message_start", "message":{"id":"live", "role":"assistant", "usage":{}}}),
    ));
    tab.feed(&frame(json!({
        "type":"content_block_start", "index":0, "content_block":{"type":"text", "text":"Start\n\n"}
    })));
}

fn after_frame(work: impl FnOnce() + 'static) {
    term_base::main_thread::after_frame(work);
    term_base::main_after(Duration::from_millis(16), || {});
}
