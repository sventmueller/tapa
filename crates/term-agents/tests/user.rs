//! What the driver offers the user's session, against the stand-ins of each path: the message's id a prompt went out
//! with, Claude's answers with an edited input and rules to keep, answers by any option a request offers, messages
//! into the running turn, the workspace's trust on the command line, a rewind at launch, and the typed failure of
//! each path.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use serde_json::json;
use support::{Harness, claude, claude_args, claude_read, folder, logged, recorded, stand_in, stand_in_env, waiting};
use term_agents::{
    AgentPath, AnswerError, Driver, Launch, Outcome, Prompt, Registry, SessionId, Signal, SubscriberId, TurnId,
    UserChoices, UserRule,
};
use term_claude::PermissionAnswer;
use term_thread::{EntryKind, Lane, PermissionOption, RuleScope, UserMessage};

fn user() -> UserChoices {
    UserChoices::default()
}

/// Starts the user's session `launch` describes and follows it.
fn start(harness: &mut Harness, launch: Launch) -> (SessionId, SubscriberId) {
    let session = harness.registry.start(launch, Box::new(UserRule)).unwrap();
    let watcher = harness.follow(session);
    (session, watcher)
}

fn send(harness: &mut Harness, session: SessionId, text: &str) -> TurnId {
    harness
        .registry
        .get_mut(session)
        .unwrap()
        .send(Prompt::new(text))
        .unwrap()
}

fn message(text: &str) -> UserMessage {
    UserMessage {
        text: text.into(),
        ..UserMessage::default()
    }
}

/// Waits until `watcher` heard of `count` requests left to the user; the id of the last.
fn nth_request(harness: &mut Harness, watcher: SubscriberId, count: usize) -> String {
    harness.until(watcher, "a request for the user", |s| {
        s.iter().filter(|s| matches!(s, Signal::Waiting { .. })).count() >= count
    });
    waiting(&harness.signals(watcher)).unwrap()
}

/// Where the permission request `request` offers the option `wanted` holds for.
fn offered(driver: &Driver, request: &str, wanted: impl Fn(&PermissionOption) -> bool) -> usize {
    (driver.thread().pending_requests().into_iter())
        .find_map(|e| match &e.kind {
            EntryKind::PermissionRequest(r) if r.request_id == request => r.options.iter().position(&wanted),
            _ => None,
        })
        .expect("the request offers the option")
}

/// Whether an answer of the agent in `session` holds `text`.
fn says(registry: &Registry, session: SessionId, text: &str) -> bool {
    let thread = registry.get(session).unwrap().thread();
    (thread.entries(&Lane::Main).iter())
        .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown.contains(text)))
}

/// The signals of a user's session `launch` describes until one `done` holds for came.
fn heard_until(launch: Launch, done: impl Fn(&Signal) -> bool) -> Vec<Signal> {
    let mut harness = Harness::new();
    let (_, watcher) = start(&mut harness, launch);
    harness.until(watcher, "the typed failure", |s| s.iter().any(&done));
    harness.signals(watcher)
}

/// Asserts that a failure in the path's words came before the typed one `typed` holds for.
fn typed_after_words(signals: &[Signal], typed: impl Fn(&Signal) -> bool) {
    let words = signals.iter().position(|s| matches!(s, Signal::Failed(_)));
    let typed = signals.iter().position(typed);
    assert!(matches!((words, typed), (Some(w), Some(t)) if w < t), "{signals:?}");
}

#[test]
fn claude_hands_on_the_message_id_and_takes_no_checkpoint() {
    let scratch = folder("claude-delivered");
    let (path, env) = claude(&scratch, &recorded("api_retry.ndjson", None), &[], false);
    let launch = Launch::new(path, &*scratch, env, user());
    let mut harness = Harness::new();
    let (session, watcher) = start(&mut harness, launch);
    let turn = send(&mut harness, session, "Fix the build\nand test it");
    let end = harness.end_of(watcher, turn);
    assert_eq!(end.outcome, Outcome::Succeeded);
    assert!(end.checkpoint.is_none());
    let delivered = harness.signals(watcher).into_iter().find_map(|s| match s {
        Signal::Delivered {
            turn: t,
            sent: Ok(Some(id)),
            checkpoint: None,
        } if t == turn => Some(id),
        _ => None,
    });
    let id = delivered.expect("the prompt went out with its id");
    harness.until_state("the message on the wire", |_| claude_read(&scratch).contains(&id));
}

#[test]
fn an_untrusted_workspace_starts_claude_and_codex_without_the_projects_configuration() {
    for trusted in [false, true] {
        let scratch = folder(&format!("claude-trust-{trusted}"));
        let (path, env) = claude(&scratch, &recorded("api_retry.ndjson", None), &[], false);
        let mut launch = Launch::new(path, &*scratch, env, user());
        launch.trusted = trusted;
        let mut harness = Harness::new();
        let (session, watcher) = start(&mut harness, launch);
        let turn = send(&mut harness, session, "Hello");
        harness.end_of(watcher, turn);
        let args = claude_args(&scratch);
        for flag in ["--setting-sources=user", "--strict-mcp-config"] {
            assert_eq!(args.iter().any(|a| a == flag), !trusted, "{flag} in {args:?}");
        }
    }
    let scratch = folder("codex-untrusted");
    let log = scratch.join("sent.ndjson");
    let args = scratch.join("args.ndjson");
    let mut env = stand_in_env("codex", &log);
    env.push(("CODEX_TEST_ARGS".into(), args.to_string_lossy().into_owned()));
    let path = AgentPath::Codex {
        program: stand_in(),
        version: None,
        shared: false,
    };
    let mut harness = Harness::new();
    let (session, watcher) = start(&mut harness, Launch::new(path, &*scratch, env, user()));
    harness.until(watcher, "the open session", |s| s.contains(&Signal::Ready));
    let started = std::fs::read_to_string(&args).unwrap();
    assert!(started.contains(r#"trust_level = \"untrusted\""#), "{started}");
    harness.registry.get_mut(session).unwrap().stop();
}

/// The signals of a Claude session that plays `frames` after its prompt, resuming `s-old` at a rewind when `rewind`,
/// until a typed failure came; and the arguments `claude` was started with.
fn claude_failure(name: &str, frames: Vec<String>, rewind: bool) -> (Vec<Signal>, Vec<String>) {
    let scratch = folder(name);
    let (path, env) = claude(&scratch, &frames, &[], false);
    let AgentPath::Claude { program, .. } = path else {
        unreachable!("the stand-in is a claude")
    };
    let rewind = rewind.then(|| term_thread::Rewind {
        fork_at: "m-kept".into(),
        drops_turn: "p-dropped".into(),
    });
    let resume = rewind.as_ref().map(|_| "s-old".to_string());
    let path = AgentPath::Claude {
        program,
        remote_control: None,
        rewind,
    };
    let mut launch = Launch::new(path, &*scratch, env, user());
    launch.resume = resume;
    let mut harness = Harness::new();
    let (session, watcher) = start(&mut harness, launch);
    send(&mut harness, session, "Reply with the single word three.");
    harness.until(watcher, "Claude's typed failure", |s| {
        s.iter().any(|s| matches!(s, Signal::ClaudeFailed(_)))
    });
    (harness.signals(watcher), claude_args(&scratch))
}

#[test]
fn claude_rewinds_at_launch_and_reports_a_refused_rewind_and_a_missing_sign_in_typed() {
    let (signals, args) = claude_failure("claude-rewind", recorded("recorded_rewind_rejected.ndjson", None), true);
    for wanted in [
        "--resume=s-old",
        "--resume-session-at=m-kept",
        "--resume-drops-turn=p-dropped",
        "--fork-session",
    ] {
        assert!(args.iter().any(|a| a == wanted), "{wanted} in {args:?}");
    }
    typed_after_words(&signals, |s| {
        matches!(s, Signal::ClaudeFailed(term_claude::SessionError::RewindRejected(_)))
    });
    let (signals, args) = claude_failure("claude-signed-out", recorded("recorded_signed_out.ndjson", None), false);
    assert!(!args.iter().any(|a| a.starts_with("--resume")), "{args:?}");
    typed_after_words(&signals, |s| {
        matches!(s, Signal::ClaudeFailed(term_claude::SessionError::SignInRequired(_)))
    });
}

#[test]
fn claude_takes_an_edited_input_and_allows_destructive_git_once_but_never_for_good() {
    let scratch = folder("claude-permission");
    let ask = json!({"type": "control_request", "request_id": "perm-rm", "request": {"subtype": "can_use_tool",
        "tool_name": "Bash", "input": {"command": "git clean -fd", "description": "Remove untracked files"},
        "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "git clean:*"}],
                                    "behavior": "allow", "destination": "localSettings"}],
        "tool_use_id": "toolu_rm"}});
    let (path, env) = claude(&scratch, &[ask.to_string()], &[], false);
    let mut harness = Harness::new();
    let (session, watcher) = start(&mut harness, Launch::new(path, &*scratch, env, user()));
    send(&mut harness, session, "Clean up");
    assert_eq!(nth_request(&mut harness, watcher, 1), "perm-rm");
    let driver = harness.registry.get_mut(session).unwrap();
    let for_good = PermissionAnswer::Allow {
        updated_input: None,
        always: vec![0],
    };
    assert!(matches!(
        driver.answer_permission_as("perm-rm", for_good),
        Err(AnswerError::Path(_))
    ));
    let elsewhere = PermissionAnswer::Allow {
        updated_input: None,
        always: Vec::new(),
    };
    assert!(matches!(
        driver.answer_permission_as("perm-none", elsewhere),
        Err(AnswerError::NoRequest(_))
    ));
    let edited = PermissionAnswer::Allow {
        updated_input: Some(json!({"command": "git clean -n", "description": "Show what would go"})),
        always: Vec::new(),
    };
    driver.answer_permission_as("perm-rm", edited).unwrap();
    harness.until_state("the edited input on the wire", |_| {
        let read = claude_read(&scratch);
        read.contains("perm-rm") && read.contains("git clean -n")
    });
    assert!(!claude_read(&scratch).contains("updatedPermissions"), "no rule kept");
}

#[test]
fn a_users_codex_session_takes_any_option_offered_no_rule_and_messages_into_its_running_turn() {
    let scratch = folder("codex-user-calls");
    let log = scratch.join("sent.ndjson");
    let path = AgentPath::Codex {
        program: stand_in(),
        version: None,
        shared: false,
    };
    let mut harness = Harness::new();
    let launch = Launch::new(path, &*scratch, stand_in_env("codex", &log), user());
    let (session, watcher) = start(&mut harness, launch);
    let turn = send(&mut harness, session, "list the files");
    let command = nth_request(&mut harness, watcher, 1);
    let driver = harness.registry.get_mut(session).unwrap();
    let for_the_session = offered(driver, &command, |o| {
        matches!(
            o,
            PermissionOption::AllowAlways {
                scope: RuleScope::Session,
                ..
            }
        )
    });
    driver.answer_option(&command, Some(for_the_session)).unwrap();
    let file = nth_request(&mut harness, watcher, 2);
    let driver = harness.registry.get_mut(session).unwrap();
    let edited = PermissionAnswer::Allow {
        updated_input: Some(json!({})),
        always: Vec::new(),
    };
    assert!(
        matches!(driver.answer_permission_as(&file, edited), Err(AnswerError::Path(_))),
        "only Claude takes an edited input"
    );
    let refused = PermissionAnswer::Deny {
        message: "Not this file".into(),
        interrupt: false,
    };
    driver.answer_permission_as(&file, refused).unwrap();
    assert_eq!(harness.end_of(watcher, turn).outcome, Outcome::Succeeded);
    let decision = |id: u64| {
        (logged(&log).into_iter())
            .find(|l| l["id"] == id && l.get("result").is_some())
            .map(|l| l["result"]["decision"].clone())
    };
    assert_eq!(decision(900), Some(json!("acceptForSession")));
    assert_eq!(decision(901), Some(json!("decline")));

    let slow = send(&mut harness, session, "slow please");
    harness.until_state("the running turn", |r| says(r, session, "Working"));
    let driver = harness.registry.get_mut(session).unwrap();
    assert_eq!(driver.steer(message("and mind the tests")), Ok(None));
    assert_eq!(driver.queued(), 0, "it went into the running turn, not into the queue");
    harness.until_state("the message in the running turn", |_| {
        (logged(&log).iter()).any(|l| l["method"] == "turn/steer" && l.to_string().contains("mind the tests"))
    });
    harness.registry.get_mut(session).unwrap().stop_turn();
    assert_eq!(harness.end_of(watcher, slow).outcome, Outcome::Cancelled);
}

#[test]
fn every_path_follows_its_failure_in_words_with_the_typed_error() {
    let scratch = folder("typed-failures");
    let mut env = stand_in_env("codex", &scratch.join("codex.ndjson"));
    env.push(("CODEX_TEST_SCENARIO".into(), "signin".into()));
    let codex = AgentPath::Codex {
        program: stand_in(),
        version: None,
        shared: false,
    };
    let signals = heard_until(Launch::new(codex, &*scratch, env, user()), |s| {
        matches!(s, Signal::CodexFailed(_))
    });
    typed_after_words(&signals, |s| {
        matches!(s, Signal::CodexFailed(term_codex::SessionError::SignIn))
    });

    let mut env = stand_in_env("opencode", &scratch.join("opencode.ndjson"));
    env.push((term_opencode::test_server::SCENARIO_VARIABLE.into(), "nomodel".into()));
    let opencode = AgentPath::OpenCode {
        program: stand_in(),
        version: None,
    };
    let signals = heard_until(Launch::new(opencode, &*scratch, env, user()), |s| {
        matches!(s, Signal::OpenCodeFailed(_))
    });
    typed_after_words(&signals, |s| {
        matches!(s, Signal::OpenCodeFailed(term_opencode::SessionError::SignIn))
    });

    let env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("SESSION_STAND_IN".to_string(), "acp".to_string()),
    ];
    let acp = term_acp::Launch::new(
        "acp-test-agent",
        "Test Agent",
        stand_in(),
        vec!["auth".into()],
        env,
        &*scratch,
    );
    let mut launch = Launch::new(AgentPath::Acp(Box::new(acp)), &*scratch, Vec::new(), user());
    launch.trusted = true;
    let signals = heard_until(launch, |s| matches!(s, Signal::AcpFailed(_)));
    typed_after_words(&signals, |s| {
        matches!(s, Signal::AcpFailed(term_acp::SessionError::SignIn))
    });
}
