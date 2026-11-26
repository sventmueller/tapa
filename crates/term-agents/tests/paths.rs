//! The driver over the real sessions of each path, against the stand-ins of the session crates and a scripted
//! `claude`: every kind of turn end, requests answered by the user, a schema for Codex's answer, a lost completion
//! event of OpenCode (C62), and an ACP agent that cannot start in an untrusted workspace.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use serde_json::{Value, json};
use support::{Harness, claude, failed_with, folder, logged, recorded, stand_in, stand_in_env, waiting};
use term_agents::rule::UserRule;
use term_agents::{
    AgentPath, AnswerError, Launch, LaunchError, Outcome, Prompt, Registry, SessionId, Signal, StreamState, UserChoices,
};
use term_thread::{EntryKind, Lane};

fn user() -> UserChoices {
    UserChoices::default()
}

fn start(harness: &mut Harness, launch: Launch) -> SessionId {
    harness.registry.start(launch, Box::new(UserRule)).unwrap()
}

fn send(harness: &mut Harness, session: SessionId, prompt: Prompt) -> term_agents::TurnId {
    harness.registry.get_mut(session).unwrap().send(prompt).unwrap()
}

/// Allows every request that waits for the user until `turn` ends.
fn allow_until_end(
    harness: &mut Harness,
    session: SessionId,
    watcher: term_agents::SubscriberId,
    turn: term_agents::TurnId,
) -> Outcome {
    let mut answered = waits(&harness.signals(watcher));
    loop {
        harness.until(watcher, "a request or the end", |s| {
            support::ended(s, turn).is_some() || waits(s) > answered
        });
        let signals = harness.signals(watcher);
        if let Some(end) = support::ended(&signals, turn) {
            return end.outcome;
        }
        let request = waiting(&signals).unwrap();
        harness
            .registry
            .get_mut(session)
            .unwrap()
            .answer_permission(&request, true, None)
            .unwrap();
        answered += 1;
    }
}

fn waits(signals: &[Signal]) -> usize {
    signals.iter().filter(|s| matches!(s, Signal::Waiting { .. })).count()
}

/// Waits until the agent's answer of the turn that runs shows `text`.
fn shows(harness: &mut Harness, session: SessionId, text: &str) {
    harness.until_state(&format!("the answer {text}"), |r: &Registry| {
        r.get(session)
            .unwrap()
            .thread()
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown.contains(text)))
    });
}

#[test]
fn codex_ends_each_kind_of_turn_and_the_user_answers_its_requests() {
    let scratch = folder("codex-user");
    let log = scratch.join("sent.ndjson");
    let path = AgentPath::Codex {
        program: stand_in(),
        version: None,
        shared: false,
    };
    let mut harness = Harness::new();
    let session = start(
        &mut harness,
        Launch::new(path, &*scratch, stand_in_env("codex", &log), user()),
    );
    let watcher = harness.follow(session);
    let first = send(&mut harness, session, Prompt::new("list the files"));
    assert_eq!(
        allow_until_end(&mut harness, session, watcher, first),
        Outcome::Succeeded
    );
    let failing = send(&mut harness, session, Prompt::new("please fail"));
    let outcome = harness.end_of(watcher, failing).outcome;
    assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    let slow = send(&mut harness, session, Prompt::new("slow please"));
    shows(&mut harness, session, "Working");
    harness.registry.get_mut(session).unwrap().stop_turn();
    assert_eq!(harness.end_of(watcher, slow).outcome, Outcome::Cancelled);
    let crash = send(&mut harness, session, Prompt::new("crash now"));
    assert_eq!(harness.end_of(watcher, crash).outcome, Outcome::ConnectionLost);
    assert!(logged(&log).iter().any(|l| l["method"] == "turn/start"));
}

#[test]
fn a_codex_prompt_carries_the_schema_of_its_answer() {
    let scratch = folder("codex-schema");
    let log = scratch.join("sent.ndjson");
    let path = AgentPath::Codex {
        program: stand_in(),
        version: None,
        shared: false,
    };
    let mut harness = Harness::new();
    let session = start(
        &mut harness,
        Launch::new(path, &*scratch, stand_in_env("codex", &log), user()),
    );
    let watcher = harness.follow(session);
    let first = send(&mut harness, session, Prompt::new("list the files"));
    assert_eq!(
        allow_until_end(&mut harness, session, watcher, first),
        Outcome::Succeeded
    );
    let mut prompt = Prompt::new("answer as JSON");
    prompt.schema = Some(json!({"type": "object", "properties": {"verdict": {"type": "string"}}}));
    let second = send(&mut harness, session, prompt);
    allow_until_end(&mut harness, session, watcher, second);
    let sent = logged(&log);
    let turns: Vec<&Value> = sent.iter().filter(|l| l["method"] == "turn/start").collect();
    assert_eq!(turns.len(), 2);
    assert!(turns[0]["params"].get("outputSchema").is_none());
    assert_eq!(turns[1]["params"]["outputSchema"]["type"], "object");
}

#[test]
fn opencode_reports_a_lost_completion_event_ends_the_users_turn_and_reads_the_session_again() {
    let scratch = folder("opencode-gap");
    let log = scratch.join("sent.ndjson");
    let path = AgentPath::OpenCode {
        program: stand_in(),
        version: None,
    };
    let mut harness = Harness::new();
    let session = start(
        &mut harness,
        Launch::new(path, &*scratch, stand_in_env("opencode", &log), user()),
    );
    let watcher = harness.follow(session);
    let gap = send(&mut harness, session, Prompt::new("mind the gap"));
    let end = harness.end_of(watcher, gap);
    assert_eq!(end.outcome, Outcome::Succeeded);
    // Nothing holds the turn's end for the gap, so the server may be read again only after it.
    harness.until(watcher, "the session was read again", |s| s.contains(&Signal::Resynced));
    let signals = harness.signals(watcher);
    let at = |wanted: &Signal| signals.iter().position(|s| s == wanted);
    let lost = at(&Signal::StreamGap).expect("the gap is reported");
    let caught_up = at(&Signal::Resynced).expect("the session was read again");
    assert!(lost < caught_up, "{signals:?}");
    let driver = harness.registry.get(session).unwrap();
    assert_eq!(*driver.stream(), StreamState::Live);
    assert!(
        driver.opencode().is_some(),
        "an OpenCode session offers what only OpenCode has"
    );
    assert!(driver.codex().is_none() && driver.acp().is_none());
    let answered =
        driver.thread().entries(&Lane::Main).iter().any(
            |e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == term_opencode::test_server::GAP_ANSWER),
        );
    assert!(answered);
    let unknown = (harness.registry.get_mut(session).unwrap()).answer_option("no-such-request", Some(0));
    assert!(matches!(unknown, Err(AnswerError::NoRequest(id)) if id == "no-such-request"));
    let failing = send(&mut harness, session, Prompt::new("fail now"));
    assert!(failed_with(
        &harness.end_of(watcher, failing).outcome,
        "Invalid API key"
    ));
    let crash = send(&mut harness, session, Prompt::new("crash please"));
    assert_eq!(harness.end_of(watcher, crash).outcome, Outcome::ConnectionLost);
}

#[test]
fn an_acp_agent_that_cannot_skip_the_projects_configuration_starts_nothing_untrusted() {
    let scratch = folder("acp-untrusted");
    let mut registry = Harness::new().registry;
    let env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("SESSION_STAND_IN".to_string(), "acp".to_string()),
    ];
    let acp = term_acp::Launch::new("gemini", "Gemini CLI", stand_in(), Vec::new(), env, &*scratch);
    let launch = Launch::new(AgentPath::Acp(Box::new(acp)), &*scratch, Vec::new(), user());
    let refused = registry.start(launch, Box::new(UserRule)).err();
    assert!(
        matches!(&refused, Some(LaunchError::Untrusted(why)) if why.contains("Gemini CLI")),
        "{refused:?}"
    );
    assert!(registry.is_empty());
}

fn claude_turn(name: &str, frames: Vec<String>, after: Vec<String>, exit: bool, stop: bool) -> Outcome {
    let scratch = folder(name);
    let (path, env) = claude(&scratch, &frames, &after, exit);
    let mut harness = Harness::new();
    let session = start(&mut harness, Launch::new(path, &*scratch, env, user()));
    let watcher = harness.follow(session);
    let turn = send(&mut harness, session, Prompt::new("Tell me a story"));
    if stop {
        shows(&mut harness, session, "Once upon a time");
        harness.registry.get_mut(session).unwrap().stop_turn();
    }
    harness.end_of(watcher, turn).outcome
}

#[test]
fn claude_ends_each_kind_of_turn() {
    let succeeded = claude_turn(
        "claude-ok",
        recorded("api_retry.ndjson", None),
        Vec::new(),
        false,
        false,
    );
    assert_eq!(succeeded, Outcome::Succeeded);
    let signed_out = recorded("recorded_signed_out.ndjson", None);
    let failed = claude_turn("claude-failed", signed_out, Vec::new(), false, false);
    assert!(failed_with(&failed, "/login"), "{failed:?}");
    let story = recorded("interrupt.ndjson", Some(r#""subtype":"interrupt""#));
    let result: Vec<String> = recorded("interrupt.ndjson", None)
        .into_iter()
        .filter(|l| l.contains(r#""type":"result""#))
        .collect();
    let cancelled = claude_turn("claude-cancelled", story.clone(), result, false, true);
    assert_eq!(cancelled, Outcome::Cancelled);
    let lost = claude_turn("claude-lost", story, Vec::new(), true, false);
    assert_eq!(lost, Outcome::ConnectionLost);
}
