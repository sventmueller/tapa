//! The driver over the scripted session: a user's turn with its requests, every kind of turn end, background work,
//! the fan-out to a slow subscriber at a high event rate, steering, stopping a turn the agent began by itself, and
//! the end of a session nobody follows.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use std::time::Duration;

use serde_json::json;
use support::{Harness, failed_with, folder};
use term_agents::{
    AgentPath, End, Launch, Outcome, Prompt, Registry, RequestKind, Script, ScriptTurn, SendError, SessionId, Signal,
    Step, UserChoices, UserRule,
};
use term_thread::{Answer, EntryKind, Lane, Notice, Status, UserMessage};

fn scripted(harness: &mut Harness, folder: &std::path::Path, turns: Vec<ScriptTurn>) -> SessionId {
    let script = Script {
        turns,
        pace: Duration::ZERO,
    };
    let launch = Launch::new(AgentPath::Scripted(script), folder, Vec::new(), UserChoices::default());
    harness.registry.start(launch, Box::new(UserRule)).unwrap()
}

/// The requests that waited for the user, in order.
fn waited(signals: &[Signal]) -> Vec<(String, RequestKind)> {
    signals
        .iter()
        .filter_map(|s| match s {
            Signal::Waiting { request, kind } => Some((request.clone(), *kind)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_users_turn_writes_in_its_folder_and_leaves_each_request_to_the_user() {
    let scratch = folder("user-turn");
    let turn = ScriptTurn::new(
        vec![
            Step::Say("Reading.".into()),
            Step::Write {
                path: "src/lib.rs".into(),
                contents: "pub fn f() {}\n".into(),
            },
            Step::Ask {
                tool: "Bash".into(),
                input: json!({"command": "ls"}),
            },
            Step::Question("Which name?".into()),
            Step::Say("Done.".into()),
        ],
        End::Succeed,
    );
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, vec![turn]);
    let watcher = harness.follow(session);
    let first = harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new("Build it"))
        .unwrap();
    harness.until(watcher, "the permission request", |s| !waited(s).is_empty());
    let (permission, kind) = waited(&harness.signals(watcher))[0].clone();
    assert_eq!(kind, RequestKind::Permission);
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(driver.waits_for_user());
    driver.answer_permission(&permission, true, None).unwrap();
    harness.until(watcher, "the question", |s| waited(s).len() == 2);
    let (question, kind) = waited(&harness.signals(watcher))[1].clone();
    assert_eq!(kind, RequestKind::Question);
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(
        driver.answer_permission(&question, true, None).is_err(),
        "a question takes no permission"
    );
    let answer = Answer {
        question: "Which name?".into(),
        answer: "f".into(),
    };
    driver.answer_questions(&question, vec![answer]).unwrap();
    let end = harness.end_of(watcher, first);
    assert_eq!(end.outcome, Outcome::Succeeded);
    assert!(end.checkpoint.is_none(), "no checkpoints are taken");
    assert_eq!(
        std::fs::read_to_string(scratch.join("src/lib.rs")).unwrap(),
        "pub fn f() {}\n"
    );
    let signals = harness.signals(watcher);
    assert!(
        !signals.iter().any(|s| matches!(s, Signal::Decided { .. })),
        "the user's rule decides nothing"
    );
    let started = signals.iter().position(|s| *s == Signal::TurnStarted(first)).unwrap();
    let ended = signals.iter().position(|s| matches!(s, Signal::TurnEnded(_))).unwrap();
    assert!(started < ended);
}

#[test]
fn every_kind_of_turn_end_is_typed() {
    let scratch = folder("endings");
    let turns = vec![
        ScriptTurn::new(vec![Step::Say("Trying.".into())], End::Fail("the model refused".into())),
        ScriptTurn::new(vec![Step::Say("Thinking.".into())], End::Hang),
        ScriptTurn::new(vec![Step::Say("Done.".into())], End::Succeed),
        ScriptTurn::new(vec![Step::Say("Working".into())], End::Crash("the agent died".into())),
    ];
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, turns);
    let watcher = harness.follow(session);
    let send = |harness: &mut Harness, text: &str| {
        harness
            .registry
            .get_mut(session)
            .unwrap()
            .send(Prompt::new(text))
            .unwrap()
    };
    let failing = send(&mut harness, "fail");
    assert!(failed_with(
        &harness.end_of(watcher, failing).outcome,
        "the model refused"
    ));
    let hanging = send(&mut harness, "hang");
    harness.until(watcher, "the hanging turn", |s| {
        s.contains(&Signal::TurnStarted(hanging))
    });
    harness.registry.get_mut(session).unwrap().stop_turn();
    assert_eq!(harness.end_of(watcher, hanging).outcome, Outcome::Cancelled);
    let fine = send(&mut harness, "fine");
    let queued = send(&mut harness, "crash");
    assert_eq!(harness.end_of(watcher, fine).outcome, Outcome::Succeeded);
    assert_eq!(harness.end_of(watcher, queued).outcome, Outcome::ConnectionLost);
    harness.until(watcher, "the end of the process", |s| {
        s.iter()
            .any(|s| matches!(s, Signal::Ended { stderr } if stderr == "the agent died"))
    });
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(driver.is_ended());
    assert_eq!(driver.send(Prompt::new("hello?")), Err(SendError::Ended));
    let mut prompt = Prompt::new("with a schema");
    prompt.schema = Some(json!({"type": "object"}));
    assert!(matches!(
        driver.send(prompt),
        Err(SendError::Ended | SendError::NoSchema(_))
    ));
}

#[test]
fn background_work_counts_until_it_is_stopped_and_stopping_the_session_cancels_what_waits() {
    let scratch = folder("background");
    let turns = vec![
        ScriptTurn::new(vec![Step::Background("dev server".into())], End::Succeed),
        ScriptTurn::new(Vec::new(), End::Hang),
    ];
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, turns);
    let watcher = harness.follow(session);
    let first = harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new("serve"))
        .unwrap();
    let end = harness.end_of(watcher, first);
    assert_eq!((end.outcome, end.running_work), (Outcome::Succeeded, 1));
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(driver.is_working());
    assert_eq!(driver.stop_background(), 1);
    harness.until_state("the stopped server", |r| r.get(session).unwrap().running_work() == 0);
    assert!(harness.registry.working().is_empty());
    let driver = harness.registry.get_mut(session).unwrap();
    let hanging = driver.send(Prompt::new("hang")).unwrap();
    let waiting = driver.send(Prompt::new("never sent")).unwrap();
    harness.until(watcher, "the hanging turn", |s| {
        s.contains(&Signal::TurnStarted(hanging))
    });
    assert_eq!(harness.registry.working(), [session]);
    assert_eq!(harness.registry.working_in(&scratch), [session]);
    let driver = harness.registry.remove(session).unwrap();
    assert!(driver.is_ended());
    assert!(harness.registry.is_empty());
    let _ = waiting;
}

#[test]
fn a_slow_subscriber_at_a_high_event_rate_gets_every_control_event_in_order_and_a_redraw_for_its_backlog() {
    let scratch = folder("fan-out");
    let mut steps: Vec<Step> = (0..5000).map(|n| Step::Say(format!("line {n}"))).collect();
    steps.push(Step::Stream {
        text: "x".into(),
        chunks: 20_000,
    });
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, vec![ScriptTurn::new(steps, End::Succeed)]);
    let fast = harness.follow(session);
    let slow = harness.registry.get_mut(session).unwrap().subscribe();
    assert!(
        harness.take(session, slow).unwrap().is_redraw(),
        "it starts with the snapshot"
    );
    let turn = harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new("flood"))
        .unwrap();
    harness.until(fast, "the turn", |s| s.contains(&Signal::TurnStarted(turn)));
    let late = harness.registry.get_mut(session).unwrap().subscribe();
    let joined = harness.take(session, late).unwrap();
    assert!(joined.is_redraw(), "a view opened mid-run starts from the snapshot");
    assert!(harness.registry.get_mut(session).unwrap().unsubscribe(late));
    let end = harness.end_of(fast, turn);
    assert_eq!(end.outcome, Outcome::Succeeded, "closing a view ends no session");
    let backlog = harness.take(session, slow).unwrap();
    assert!(backlog.is_redraw(), "a backlog of {} changes", backlog.changes.len());
    let control = |signals: &[Signal]| -> Vec<Signal> {
        signals
            .iter()
            .filter(|s| matches!(s, Signal::Ready | Signal::TurnStarted(_) | Signal::TurnEnded(_)))
            .cloned()
            .collect()
    };
    let wanted = vec![Signal::Ready, Signal::TurnStarted(turn), Signal::TurnEnded(end)];
    assert_eq!(control(&backlog.signals), wanted);
    assert_eq!(control(&harness.signals(fast)), wanted);
    let said = backlog
        .thread
        .entries(&Lane::Main)
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::AgentMessage(_)))
        .count();
    assert_eq!(said, 5001);
    let streamed = backlog
        .thread
        .entries(&Lane::Main)
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EntryKind::AgentMessage(m) => Some(m.markdown.len()),
            _ => None,
        });
    assert_eq!(streamed, Some(20_000));
    assert!(
        harness.take(session, slow).unwrap().signals.is_empty(),
        "each signal once"
    );
}

#[test]
fn a_view_that_joins_mid_run_writes_into_the_queue_stops_one_task_and_leaves_without_ending_the_session() {
    let scratch = folder("attach");
    let turns = vec![
        ScriptTurn::new(vec![Step::Background("watcher".into())], End::Succeed),
        ScriptTurn::new(vec![Step::Say("Semicolons too.".into())], End::Succeed),
    ];
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, turns);
    let runtime = harness.follow(session);
    let first = harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new("build"))
        .unwrap();
    harness.end_of(runtime, first);
    let tab = harness.follow(session);
    assert!(
        harness.batches[&tab][0].is_redraw(),
        "a view that joins starts with the snapshot"
    );
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(driver.claude_capabilities().is_none());
    assert!(
        driver.set_model("claude-opus-5-5").is_err(),
        "the scripted session keeps its model"
    );
    let task = driver.thread().background()[0].id.clone();
    assert!(driver.stop_task(&task));
    let asked = driver.send(Prompt::new("allow semicolons")).unwrap();
    let end = harness.end_of(tab, asked);
    assert_eq!(end.outcome, Outcome::Succeeded);
    let driver = harness.registry.get_mut(session).unwrap();
    assert_eq!(driver.subscribers(), 2);
    assert!(driver.unsubscribe(tab));
    assert_eq!(driver.subscribers(), 1);
    assert!(!driver.is_ended(), "the session runs on without the view");
}

fn message(text: &str) -> UserMessage {
    UserMessage {
        text: text.into(),
        ..UserMessage::default()
    }
}

fn says(registry: &Registry, session: SessionId, text: &str) -> bool {
    let thread = registry.get(session).unwrap().thread();
    (thread.entries(&Lane::Main).iter()).any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == text))
}

#[test]
fn steering_reaches_the_user_s_agent_at_once() {
    let scratch = folder("steer");
    let mut harness = Harness::new();
    let answer = ScriptTurn::new(vec![Step::Say("Steered.".into())], End::Succeed);
    let users = scripted(&mut harness, &scratch, vec![answer]);
    let watcher = harness.follow(users);
    harness.until(watcher, "the open session", |s| s.contains(&Signal::Ready));
    let driver = harness.registry.get_mut(users).unwrap();
    assert_eq!(driver.steer(message("Look at the tests")), Ok(None));
    assert_eq!(driver.queued(), 0, "it went to the agent, not into the queue");
    harness.until_state("the steered answer", |r| says(r, users, "Steered."));
    assert!(
        !(harness.signals(watcher).iter()).any(|s| matches!(s, Signal::TurnStarted(_))),
        "no turn of the driver"
    );

    let driver = harness.registry.get_mut(users).unwrap();
    driver.stop();
    assert_eq!(driver.steer(message("Still there?")), Err(SendError::Ended));
}

#[test]
fn stopping_also_stops_a_turn_the_agent_began_by_itself() {
    let scratch = folder("stop-own-turn");
    let own = ScriptTurn::new(vec![Step::Say("On my own.".into())], End::Hang);
    let mut harness = Harness::new();
    let session = scripted(&mut harness, &scratch, vec![own]);
    let watcher = harness.follow(session);
    harness.until(watcher, "the open session", |s| s.contains(&Signal::Ready));
    let driver = harness.registry.get_mut(session).unwrap();
    driver.steer(message("Carry on")).unwrap();
    harness.until_state("the agent's own turn", |r| {
        r.get(session).unwrap().thread().status() != Status::Idle
    });
    harness.registry.get_mut(session).unwrap().stop_turn();
    harness.until_state("the agent's own turn to stop", |r| {
        let thread = r.get(session).unwrap().thread();
        thread.status() == Status::Idle
            && (thread.entries(&Lane::Main).iter()).any(|e| matches!(e.kind, EntryKind::Notice(Notice::Interrupted)))
    });
}

#[test]
fn a_session_nobody_follows_any_more_is_stopped_and_let_go() {
    let scratch = folder("ownerless");
    let mut harness = Harness::new();
    let hanging = ScriptTurn::new(Vec::new(), End::Hang);
    let session = scripted(&mut harness, &scratch, vec![hanging]);
    let runtime = harness.follow(session);
    let tab = harness.follow(session);
    let turn = harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new("Wait"))
        .unwrap();
    harness.until(runtime, "the hanging turn", |s| s.contains(&Signal::TurnStarted(turn)));
    let driver = harness.registry.get_mut(session).unwrap();
    assert!(driver.unsubscribe(tab));
    driver.stop_turn();
    assert_eq!(harness.end_of(runtime, turn).outcome, Outcome::Cancelled);
    assert_eq!(harness.registry.ids(), [session], "one follower keeps it");
    assert!(harness.registry.get_mut(session).unwrap().unsubscribe(runtime));
    harness.until_state("the session let go", Registry::is_empty);
}
