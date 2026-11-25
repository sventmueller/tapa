//! The session against a stand-in for `claude`: a shell script that waits for `initialize`, prints canned frames
//! and keeps everything the session writes. One ignored test runs the real CLI.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use term_base::testing::scratch;

use serde_json::{Value, json};
use term_claude::control::{Backgrounded, ContextCount};
use term_claude::{
    ControlError, Effort, Event, Launch, PermissionAnswer, PermissionMode, RemoteControl, Session, SessionError,
};
use term_process::Supervisor;
use term_thread::{Decision, EntryKind, ErrorKind, Lane, Notice, RequestState, Status, Thread, ThreadId, UserMessage};

const WAIT: Duration = Duration::from_secs(10);

/// A stand-in CLI in `dir` that answers `initialize` with `account` and then prints `frames`.
fn stand_in(dir: &Path, account: Value, frames: &[Value]) -> Launch {
    let answer = json!({"commands": [], "models": [{"value": "haiku", "displayName": "Haiku"}], "account": account, "output_style": "default"});
    stand_in_answering(dir, answer, frames)
}

/// A stand-in CLI in `dir` that answers `initialize` with `answer` and then prints `frames`.
fn stand_in_answering(dir: &Path, answer: Value, frames: &[Value]) -> Launch {
    let mut lines = vec![
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-1", "response": answer}}),
    ];
    lines.extend_from_slice(frames);
    let fixture: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(dir.join("fixture.ndjson"), fixture).unwrap();
    let script = dir.join("claude");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$OUT.args\"\nread -r first\nprintf '%s\\n' \"$first\" > \"$OUT\"\ncat \"$FIXTURE\"\n[ -n \"$THEN_EXIT\" ] && exit 0\ncat >> \"$OUT\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        (
            "FIXTURE".to_string(),
            dir.join("fixture.ndjson").to_string_lossy().into_owned(),
        ),
        (
            "OUT".to_string(),
            dir.join("written.ndjson").to_string_lossy().into_owned(),
        ),
    ];
    Launch::new(script, env, dir)
}

fn start(launch: &Launch, supervisor: &Supervisor) -> (Session, Receiver<Event>) {
    let (tx, rx) = channel();
    let thread = Thread::new(ThreadId::new(), "claude", launch.cwd.clone());
    let session = Session::start(supervisor, launch, thread, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (session, rx)
}

/// Waits for an event `want` accepts.
fn wait_for(rx: &Receiver<Event>, mut want: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = rx.recv_timeout(left).expect("the awaited event came in time");
        if want(&event) {
            return event;
        }
    }
}

fn written(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("written.ndjson"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_session_answers_permissions_refuses_unknown_requests_and_ends_cleanly() {
    let dir = scratch("flow");
    let launch = stand_in(
        &dir,
        json!({"subscriptionType": "pro", "apiProvider": "firstParty"}),
        &[
            json!({"type": "system", "subtype": "init", "session_id": "s-1", "cwd": "/w", "model": "claude-haiku-5-5", "permissionMode": "default", "tools": ["Bash"], "claude_code_version": "2.1.319", "capabilities": ["interrupt_receipt_v1"]}),
            json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "toolu_x", "name": "Bash", "input": {"command": "ls"}}]}, "parent_tool_use_id": null}),
            json!({"type": "control_request", "request_id": "cli-1", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "toolu_x"}}),
            json!({"type": "control_request", "request_id": "cli-2", "request": {"subtype": "summon_dragon"}}),
            json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "text", "text": "Hello"}]}, "parent_tool_use_id": null}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| session.with_thread(|t| !t.pending_requests().is_empty()));
    assert_eq!(session.with_thread(Thread::status), Status::RequiresAction);
    assert!(!session.can_park());
    assert!(matches!(
        session.answer(
            "nope",
            PermissionAnswer::Deny {
                message: String::new(),
                interrupt: false
            }
        ),
        Err(SessionError::UnknownRequest(_))
    ));
    session
        .answer(
            "cli-1",
            PermissionAnswer::Allow {
                updated_input: Some(json!({"command": "ls -a"})),
                always: Vec::new(),
            },
        )
        .unwrap();
    session
        .send(UserMessage {
            text: "Hi there\nsecond line".into(),
            ..UserMessage::default()
        })
        .unwrap();
    let interrupt = session.interrupt();
    wait_for(&rx, |_| {
        session.with_thread(|t| {
            t.entries(&Lane::Main)
                .iter()
                .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == "Hello"))
        })
    });
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert!(session.is_ended());
    assert_eq!(interrupt.wait(), Err(ControlError::Closed));
    assert!(matches!(
        session.send(UserMessage::default()),
        Err(SessionError::Closed)
    ));

    let thread = session.snapshot();
    assert_eq!(thread.title(), "Hi there");
    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.session(), Some("s-1"));
    let Some(EntryKind::PermissionRequest(request)) = thread.entry_by_key("perm:cli-1").map(|e| &e.kind) else {
        panic!()
    };
    assert_eq!(
        request.state,
        RequestState::Decided(Decision::Allowed {
            always: false,
            edited: true
        })
    );
    let caps = session.capabilities();
    assert_eq!(caps.version.as_deref(), Some("2.1.319"));
    assert!(caps.interrupt_receipts());
    assert_eq!(caps.models[0].value, "haiku");

    let frames = written(&dir);
    let find = |pred: &dyn Fn(&Value) -> bool| frames.iter().find(|f| pred(f)).cloned();
    let init = find(&|f| f["request"]["subtype"] == "initialize").expect("initialize was sent first");
    assert_eq!(frames[0], init);
    assert_eq!(init["request"]["perTaskStopAffordance"], true);
    let allow = find(&|f| f["response"]["request_id"] == "cli-1").expect("the permission answer");
    assert_eq!(allow["response"]["response"]["updatedInput"]["command"], "ls -a");
    assert_eq!(allow["response"]["response"]["toolUseID"], "toolu_x");
    let refusal = find(&|f| f["response"]["request_id"] == "cli-2").expect("the refusal");
    assert_eq!(refusal["response"]["subtype"], "error");
    let prompt = find(&|f| f["type"] == "user").expect("the prompt");
    assert_eq!(prompt["message"]["content"][0]["text"], "Hi there\nsecond line");
    assert_eq!(prompt["origin"]["kind"], "human");
    assert!(find(&|f| f["request"]["subtype"] == "interrupt").is_some());
    let args = std::fs::read_to_string(dir.join("written.ndjson.args")).unwrap();
    assert!(args.lines().any(|a| a == "--permission-prompt-tool=stdio"));
    assert!(args.lines().any(|a| a == "--input-format=stream-json"));
}

#[test]
fn a_destructive_git_command_asks_before_a_stored_rule_or_the_mode_could_allow_it() {
    let dir = scratch("destructive");
    let hook = |id: &str, command: &str| json!({"type": "control_request", "request_id": id, "request": {"subtype": "hook_callback", "callback_id": "term-destructive-git", "tool_use_id": "toolu_r", "input": {"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": command}, "tool_use_id": "toolu_r", "permission_mode": "bypassPermissions"}}});
    // The CLI's order: the hook first; with its `ask`, the question, although `Bash(git reset:*)` is stored.
    let launch = stand_in(
        &dir,
        json!({"subscriptionType": "pro"}),
        &[
            json!({"type": "system", "subtype": "init", "session_id": "s-d", "cwd": "/w", "model": "claude-haiku-5-5", "permissionMode": "bypassPermissions", "tools": ["Bash"], "claude_code_version": "2.1.322"}),
            hook("cli-1", "git status"),
            hook("cli-2", "cd repo && git reset --hard HEAD~1"),
            json!({"type": "control_request", "request_id": "cli-3", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "cd repo && git reset --hard HEAD~1"}, "tool_use_id": "toolu_r", "decision_reason": "Destructive git command: it needs the user's approval every time.", "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Bash", "ruleContent": "git reset:*"}], "behavior": "allow", "destination": "localSettings"}]}}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| session.with_thread(|t| !t.pending_requests().is_empty()));
    let request = session.with_thread(|t| match t.entry_by_key("perm:cli-3").map(|e| &e.kind) {
        Some(EntryKind::PermissionRequest(request)) => request.clone(),
        other => panic!("{other:?}"),
    });
    assert_eq!(
        request.options,
        vec![
            term_thread::PermissionOption::AllowOnce,
            term_thread::PermissionOption::Deny
        ],
        "no rule can be stored for it"
    );
    assert!(request.default_to_no);
    session
        .answer(
            "cli-3",
            PermissionAnswer::Deny {
                message: "no".into(),
                interrupt: false,
            },
        )
        .unwrap();
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));

    let frames = written(&dir);
    let answer = |id: &str| {
        frames
            .iter()
            .find(|f| f["response"]["request_id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no answer to {id}"))
    };
    assert_eq!(
        frames[0]["request"]["hooks"]["PreToolUse"][0]["hookCallbackIds"][0],
        "term-destructive-git"
    );
    assert_eq!(answer("cli-1")["response"]["response"], json!({}));
    let asked = answer("cli-2");
    assert_eq!(asked["response"]["subtype"], "success");
    assert_eq!(
        asked["response"]["response"]["hookSpecificOutput"]["permissionDecision"],
        "ask"
    );
    assert_eq!(answer("cli-3")["response"]["response"]["behavior"], "deny");
}

#[test]
#[ignore = "runs the installed claude CLI: needs a sign-in and the network, and costs a few tokens"]
fn the_real_cli_asks_for_a_destructive_git_command_despite_a_stored_rule_and_bypass_mode() {
    let executable =
        std::env::var_os("CLAUDE_BIN").map_or_else(|| term_base::home().join(".local/bin/claude"), PathBuf::from);
    let dir = scratch("real-destructive");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(dir.join("kept.txt"), "committed\n").unwrap();
    git(&["add", "kept.txt"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "c"]);
    std::fs::write(dir.join("kept.txt"), "uncommitted work\n").unwrap();
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(
        dir.join(".claude/settings.local.json"),
        r#"{"permissions": {"allow": ["Bash(git reset:*)", "Bash(git:*)"]}}"#,
    )
    .unwrap();
    let env: Vec<(String, String)> = std::env::vars().collect();
    let mut launch = Launch::new(executable, env, &dir);
    launch.model = Some("haiku".into());
    launch.effort = Some("low".into());
    launch.persist = false;
    launch.allow_bypass = true;
    launch.permission_mode = Some("bypassPermissions".into());
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    session
        .send(UserMessage {
            text: "Use the Bash tool to run exactly this command and nothing else: git reset --hard".into(),
            ..UserMessage::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut asked = None;
    while Instant::now() < deadline && asked.is_none() {
        let _ = rx.recv_timeout(Duration::from_millis(500));
        asked = session.with_thread(|t| {
            t.pending_requests().into_iter().find_map(|e| match &e.kind {
                EntryKind::PermissionRequest(r) => Some(r.request_id.clone()),
                _ => None,
            })
        });
    }
    let request = asked.expect("the CLI asked before running git reset --hard");
    session
        .answer(
            &request,
            PermissionAnswer::Deny {
                message: "Not now.".into(),
                interrupt: true,
            },
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("kept.txt")).unwrap(),
        "uncommitted work\n"
    );
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

#[test]
fn a_cli_without_sign_in_is_reported_so_the_user_can_log_in_in_a_terminal() {
    let dir = scratch("signed-out");
    let launch = stand_in(&dir, json!({"tokenSource": "none", "apiProvider": "firstParty"}), &[]);
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    let event = wait_for(&rx, |e| matches!(e, Event::Failed(_)));
    assert!(matches!(event, Event::Failed(SessionError::SignInRequired(_))));
    session.stop(Duration::from_millis(200));
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

#[test]
fn a_refused_rewind_ends_the_session_with_its_own_error_and_no_crash_report() {
    let dir = scratch("rewind-rejected");
    let refusal =
        "Resume rejected by --resume-drops-turn: resuming at a-1 would discard entries not attributable to turn p-2";
    let mut launch = stand_in(
        &dir,
        json!({"subscriptionType": "pro"}),
        &[
            json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "result": null, "errors": [refusal], "num_turns": 0, "session_id": "s-2"}),
        ],
    );
    launch.env.push(("THEN_EXIT".into(), "1".into()));
    launch.rewind(
        "s-1",
        &term_thread::Rewind {
            fork_at: "a-1".into(),
            drops_turn: "p-2".into(),
        },
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    let event = wait_for(&rx, |e| matches!(e, Event::Failed(_)));
    assert_eq!(event, Event::Failed(SessionError::RewindRejected(refusal.into())));
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    let thread = session.snapshot();
    let errors = thread
        .entries(&Lane::Main)
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Notice(term_thread::Notice::Error { .. })))
        .count();
    assert_eq!(errors, 0, "the refusal is not reported as a crash of the process");
    let args = std::fs::read_to_string(dir.join("written.ndjson.args")).unwrap();
    assert!(args.lines().any(|a| a == "--resume-drops-turn=p-2"));
}

#[test]
fn after_a_turn_the_session_asks_how_full_the_context_is_and_keeps_the_breakdown() {
    let dir = scratch("context");
    let launch = stand_in(
        &dir,
        json!({"subscriptionType": "pro"}),
        &[
            json!({"type": "system", "subtype": "init", "session_id": "s-3", "cwd": "/w", "model": "claude-haiku-5-5", "permissionMode": "default", "tools": [], "claude_code_version": "2.1.321"}),
            json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 900, "total_cost_usd": 0.002, "modelUsage": {"claude-haiku-5-5": {"inputTokens": 4, "outputTokens": 9, "costUSD": 0.002, "contextWindow": 200000}}, "session_id": "s-3"}),
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-2", "response": {"categories": [{"id": "messages", "name": "Messages", "tokens": 1200, "kind": "used"}, {"id": "free_space", "name": "Free space", "tokens": 198800, "kind": "free"}], "totalTokens": 1200, "rawMaxTokens": 200000, "percentage": 0.6, "model": "claude-haiku-5-5"}}}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| session.with_thread(|t| t.usage().breakdown.is_some()));
    let usage = session.with_thread(|t| t.usage().clone());
    assert_eq!(usage.context_used, Some(1_200));
    assert_eq!(usage.breakdown.map(|b| (b.rows.len(), b.exact)), Some((2, false)));
    assert_eq!(usage.turns.len(), 1);
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    let asked: Vec<Value> = written(&dir)
        .into_iter()
        .filter(|f| f["request"]["subtype"] == "get_context_usage")
        .collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["request"]["detail"], "summary");
}

#[test]
fn a_missing_executable_is_reported_before_anything_starts() {
    let launch = Launch::new("/nonexistent/claude", Vec::new(), "/tmp");
    let result = Session::start(
        &Supervisor::new(),
        &launch,
        Thread::new(ThreadId::new(), "claude", "/tmp"),
        |_| {},
    );
    assert!(matches!(result, Err(SessionError::NotFound(_))));
}

#[test]
#[ignore = "runs the installed claude CLI: needs a sign-in and the network, and costs a few tokens"]
fn the_real_cli_answers_a_prompt() {
    let executable =
        std::env::var_os("CLAUDE_BIN").map_or_else(|| term_base::home().join(".local/bin/claude"), PathBuf::from);
    let dir = scratch("real");
    let env: Vec<(String, String)> = std::env::vars().collect();
    let mut launch = Launch::new(executable, env, &dir);
    launch.model = Some("haiku".into());
    launch.effort = Some("low".into());
    launch.persist = false;
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    session
        .send(UserMessage {
            text: "Reply with the single word OK.".into(),
            ..UserMessage::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let _ = rx.recv_timeout(Duration::from_millis(500));
        let done = session.with_thread(|t| {
            t.status() == Status::Idle
                && t.entries(&Lane::Main)
                    .iter()
                    .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown.contains("OK")))
        });
        if done {
            break;
        }
    }
    let thread = session.snapshot();
    assert!(thread.session().is_some());
    assert!(thread.usage().cost_usd.is_some());
    assert!(
        thread
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown.contains("OK")))
    );
    let caps = session.capabilities();
    assert!(caps.version.is_some());
    assert!(!caps.models.is_empty(), "initialize was answered");
    assert!(session.context_usage(ContextCount::Estimated).wait().is_ok());
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

/// The CLI named by `CLAUDE_BIN` against the contract the `recorded_signed_out` fixture holds, in a home of its own
/// where it finds no sign-in, so it costs nothing and needs no account: it takes the app's whole command line, answers
/// `initialize` with its version, models and the missing account, starts the session and refuses the prompt as an
/// authentication error. `cargo xtask claude-latest` runs it against the newest CLI (R7).
#[test]
#[ignore = "runs the claude CLI named by CLAUDE_BIN without a sign-in"]
fn the_cli_without_a_sign_in_keeps_the_recorded_contract() {
    let executable = PathBuf::from(std::env::var_os("CLAUDE_BIN").expect("CLAUDE_BIN names the CLI to check"));
    let dir = scratch("contract");
    let (home, project) = (dir.join("home"), dir.join("project"));
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    // Nothing of the caller's environment but PATH, so no API key or token there signs the CLI in; with its own
    // config folder the CLI also looks for a keychain item of another name than the user's.
    let env = vec![
        ("PATH".to_string(), std::env::var("PATH").unwrap_or_default()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_string(),
            home.join(".claude").to_string_lossy().into_owned(),
        ),
        (
            "TMPDIR".to_string(),
            std::env::temp_dir().to_string_lossy().into_owned(),
        ),
    ];
    let mut launch = Launch::new(executable, env, &project);
    launch.model = Some("haiku".into());
    launch.effort = Some("low".into());
    launch.persist = false;
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut signed_out = false;
    while !signed_out && Instant::now() < deadline {
        if let Ok(event) = rx.recv_timeout(Duration::from_millis(500)) {
            signed_out = matches!(event, Event::Failed(SessionError::SignInRequired(_)));
        }
    }
    assert!(signed_out, "the missing sign-in was reported");
    let caps = session.capabilities();
    let version = caps.version.clone().expect("initialize named the CLI's version");
    println!("claude {version} answered initialize with {} models", caps.models.len());
    assert!(
        caps.models.iter().any(|m| !m.value.is_empty()),
        "initialize listed models"
    );
    let account = caps.account.expect("initialize described the account");
    assert_eq!(account.token_source.as_deref(), Some("none"));
    session
        .send(UserMessage {
            text: "Reply OK.".into(),
            ..UserMessage::default()
        })
        .unwrap();
    let refused = |t: &Thread| {
        t.status() == Status::Idle
            && t.entries(&Lane::Main).iter().any(|e| {
                matches!(
                    &e.kind,
                    EntryKind::Notice(Notice::Error {
                        kind: ErrorKind::Auth,
                        ..
                    })
                )
            })
    };
    while !session.with_thread(refused) && Instant::now() < deadline {
        let _ = rx.recv_timeout(Duration::from_millis(500));
    }
    let thread = session.snapshot();
    assert!(refused(&thread), "{:?}", thread.entries(&Lane::Main));
    assert!(thread.session().is_some(), "system/init named the session");
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

fn remote_answer() -> Value {
    json!({"commands": [], "models": [], "account": {}, "output_style": "default", "remote_control_available": true, "remote_control_auto_enable": true})
}

fn remote_requests(dir: &Path) -> Vec<Value> {
    written(dir)
        .into_iter()
        .filter(|f| f.pointer("/request/subtype") == Some(&json!("remote_control")))
        .map(|f| f["request"].clone())
        .collect()
}

#[test]
fn remote_control_connects_as_the_settings_ask_recovers_and_shows_prompts_from_the_phone() {
    let dir = scratch("remote");
    let launch = stand_in_answering(
        &dir,
        remote_answer(),
        &[
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-2", "response": {"session_url": "https://claude.ai/code/s-1", "bridge_epoch": 3}}}),
            json!({"type": "system", "subtype": "bridge_state", "state": "failed", "bridge_epoch": 3, "detail": "network"}),
            json!({"type": "system", "subtype": "bridge_state", "state": "connected", "bridge_epoch": 3, "recovered": true}),
            json!({"type": "user", "uuid": "t-1", "isReplay": true, "origin": {"kind": "task-notification"}, "message": {"role": "user", "content": "a task ended"}}),
            json!({"type": "user", "uuid": "p-1", "isReplay": true, "message": {"role": "user", "content": "from the phone"}}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    let texts = |t: &Thread| -> Vec<String> {
        t.entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::UserMessage(m) => Some(m.text.clone()),
                _ => None,
            })
            .collect()
    };
    wait_for(&rx, |_| {
        session.remote_control() == RemoteControl::Connected("https://claude.ai/code/s-1".into())
            && !session.with_thread(texts).is_empty()
    });
    assert_eq!(session.with_thread(texts), ["from the phone"]);
    let sent = session.send(UserMessage {
        text: "from the app".into(),
        ..UserMessage::default()
    });
    assert!(sent.is_ok());
    session.set_remote_control(false);
    assert_eq!(session.remote_control(), RemoteControl::Off);
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(session.remote_control(), RemoteControl::Unavailable);
    assert_eq!(
        remote_requests(&dir),
        [
            json!({"subtype": "remote_control", "enabled": true}),
            json!({"subtype": "remote_control", "enabled": false})
        ]
    );
    let args = std::fs::read_to_string(dir.join("written.ndjson.args")).unwrap();
    assert!(args.lines().any(|a| a == "--replay-user-messages"));
}

#[test]
fn a_thread_that_turned_remote_control_off_stays_off_when_it_starts_again() {
    let dir = scratch("remote-off");
    let mut launch = stand_in_answering(&dir, remote_answer(), &[]);
    launch.remote_control = Some(false);
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |e| matches!(e, Event::RemoteControl));
    assert_eq!(session.remote_control(), RemoteControl::Off);
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert!(remote_requests(&dir).is_empty());
}

#[test]
fn remote_control_fails_when_its_connection_breaks_off_but_not_for_an_older_one() {
    let dir = scratch("remote-failed");
    let launch = stand_in_answering(
        &dir,
        remote_answer(),
        &[
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": "req-2", "response": {"session_url": "https://claude.ai/code/s-2", "bridge_epoch": 2}}}),
            json!({"type": "system", "subtype": "bridge_state", "state": "failed", "bridge_epoch": 1, "detail": "old"}),
            json!({"type": "system", "subtype": "bridge_state", "state": "failed", "bridge_epoch": 2, "detail": "network"}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| matches!(session.remote_control(), RemoteControl::Failed(_)));
    assert_eq!(session.remote_control(), RemoteControl::Failed("network".into()));
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
}

/// A stand-in CLI in `dir` that answers `initialize`, prints `frames` and then answers each control request with the
/// payload in `answers` for its subtype, or refuses it.
fn responder(dir: &Path, frames: &[Value], answers: &[(&str, Value)]) -> Launch {
    let mut launch = stand_in(dir, json!({"subscriptionType": "pro"}), frames);
    let folder = dir.join("answers");
    std::fs::create_dir_all(&folder).unwrap();
    for (subtype, payload) in answers {
        std::fs::write(folder.join(format!("{subtype}.json")), payload.to_string()).unwrap();
    }
    std::fs::write(
        dir.join("claude"),
        r#"#!/bin/sh
read -r first
printf '%s\n' "$first" > "$OUT"
cat "$FIXTURE"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$OUT"
  case "$line" in *'"type":"control_request"'*) ;; *) continue ;; esac
  id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
  sub=$(printf '%s' "$line" | sed -n 's/.*"subtype":"\([^"]*\)".*/\1/p')
  if [ -f "$ANSWERS/$sub.json" ]; then
    printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":%s}}\n' "$id" "$(cat "$ANSWERS/$sub.json")"
  else
    printf '{"type":"control_response","response":{"subtype":"error","request_id":"%s","error":"no %s here"}}\n' "$id" "$sub"
  fi
done
"#,
    )
    .unwrap();
    launch
        .env
        .push(("ANSWERS".into(), folder.to_string_lossy().into_owned()));
    launch
}

#[test]
fn the_settings_of_a_running_session_reach_the_cli_and_its_answers_land_on_the_thread() {
    let dir = scratch("controls");
    let launch = responder(
        &dir,
        &[
            json!({"type": "system", "subtype": "init", "session_id": "s-c", "cwd": "/w", "model": "claude-haiku-5-5", "permissionMode": "default", "tools": ["Bash"], "claude_code_version": "2.1.322"}),
            json!({"type": "system", "subtype": "background_tasks_changed", "tasks": [{"task_id": "bash1", "task_type": "local_bash", "description": "count"}], "session_id": "s-c"}),
        ],
        &[
            ("set_permission_mode", json!({})),
            ("set_model", json!({})),
            ("apply_flag_settings", json!({})),
            ("background_tasks", json!({"backgrounded": false})),
            (
                "get_task_output",
                json!({"output": "3\n2\n1\n", "total_bytes": 4096, "truncated": true}),
            ),
            (
                "get_usage",
                json!({"session": {"total_cost_usd": 0.25}, "subscription_type": "max", "rate_limits": {"five_hour": {"utilization": 0.4, "resets_at": "2026-11-13T18:00:00Z"}}}),
            ),
            (
                "rewind_files",
                json!({"canRewind": true, "filesChanged": ["src/a.rs"], "insertions": 3, "deletions": 1}),
            ),
            (
                "get_context_usage",
                json!({"categories": [{"id": "messages", "name": "Messages", "tokens": 900, "kind": "used"}], "totalTokens": 900, "rawMaxTokens": 200000, "percentage": 0.45, "model": "claude-haiku-5-5"}),
            ),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| session.with_thread(|t| !t.background().is_empty()));
    assert!(!session.can_park(), "a background shell keeps the CLI live");

    assert_eq!(session.set_permission_mode(PermissionMode::Plan).wait(), Ok(()));
    assert_eq!(session.set_model(Some("claude-opus-5-5")).wait(), Ok(()));
    assert_eq!(session.set_effort(Effort::High).wait(), Ok(()));
    session.with_thread(|t| {
        assert_eq!(t.mode(), Some("plan"));
        assert_eq!(t.model(), Some("claude-opus-5-5"));
        assert_eq!(t.effort(), Some("high"));
    });
    assert_eq!(
        session.background(Some("toolu_b")).wait(),
        Ok(Backgrounded { moved: false })
    );
    assert!(matches!(
        session.stop_task("bash1").wait(),
        Err(ControlError::Refused(why)) if why == "no stop_task here"
    ));

    let output = session.task_output("bash1").wait().unwrap();
    assert_eq!(
        (output.output.as_str(), output.total_bytes, output.truncated),
        ("3\n2\n1\n", 4096, true)
    );
    let tail = session.with_thread(|t| t.background()[0].output.clone());
    assert_eq!(tail.text.as_deref(), Some("3\n2\n1\n"));
    assert_eq!(tail.total_bytes, Some(4096));

    let usage = session.usage().wait().unwrap();
    assert_eq!(usage.session_cost_usd, Some(0.25));
    assert_eq!(usage.subscription.as_deref(), Some("max"));
    assert_eq!(usage.windows[0].utilization, Some(0.4));
    let rewind = session.preview_rewind("u-1").wait().unwrap();
    assert!(rewind.can_rewind);
    assert_eq!(rewind.files_changed, ["src/a.rs"]);
    assert_eq!(
        session.context_usage(ContextCount::Exact).wait().unwrap().total_tokens,
        900
    );
    let breakdown = session.with_thread(|t| t.usage().breakdown.clone()).unwrap();
    assert!(breakdown.exact, "a full count is kept as exact");

    let (tx, answered) = channel();
    session.set_model(None).then(move |result| {
        let _ = tx.send(result);
    });
    assert_eq!(answered.recv_timeout(WAIT).unwrap(), Ok(()));
    assert_eq!(
        session.with_thread(|t| t.model().map(str::to_string)),
        Some("claude-opus-5-5".into()),
        "returning to the default names no model"
    );

    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));
    assert_eq!(session.set_effort(Effort::Low).wait(), Err(ControlError::Closed));
    let frames = written(&dir);
    let sent = |subtype: &str| {
        frames
            .iter()
            .find(|f| f["request"]["subtype"] == subtype)
            .map(|f| f["request"].clone())
            .unwrap_or_else(|| panic!("no {subtype} was sent"))
    };
    assert_eq!(sent("set_permission_mode")["mode"], "plan");
    assert_eq!(sent("apply_flag_settings")["settings"]["effortLevel"], "high");
    assert_eq!(sent("background_tasks")["tool_use_id"], "toolu_b");
    assert_eq!(sent("stop_task")["task_id"], "bash1");
    assert_eq!(sent("rewind_files")["dry_run"], true);
    assert_eq!(sent("get_context_usage")["detail"], "full");
}

#[test]
fn questions_and_plans_are_answered_with_the_users_choice() {
    let dir = scratch("questions");
    let question = json!({"questions": [{"question": "Which color?", "header": "Color", "options": [{"label": "red", "description": ""}, {"label": "blue", "description": ""}], "multiSelect": false}]});
    let plan = json!({"plan": "## Plan\n1. Fix it"});
    let launch = responder(
        &dir,
        &[
            json!({"type": "system", "subtype": "init", "session_id": "s-q", "cwd": "/w", "model": "claude-haiku-5-5", "permissionMode": "plan", "tools": ["AskUserQuestion", "ExitPlanMode"], "claude_code_version": "2.1.322"}),
            json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "toolu_q", "name": "AskUserQuestion", "input": question}]}, "parent_tool_use_id": null}),
            json!({"type": "control_request", "request_id": "cli-q", "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion", "input": question, "tool_use_id": "toolu_q"}}),
            json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "tool_use", "id": "toolu_p", "name": "ExitPlanMode", "input": plan}]}, "parent_tool_use_id": null}),
            json!({"type": "control_request", "request_id": "cli-p", "request": {"subtype": "can_use_tool", "tool_name": "ExitPlanMode", "input": plan, "tool_use_id": "toolu_p"}}),
            json!({"type": "control_request", "request_id": "cli-gone", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "toolu_g"}}),
            json!({"type": "control_cancel_request", "request_id": "cli-gone"}),
            json!({"type": "assistant", "message": {"id": "m3", "content": [{"type": "text", "text": "after-cancel"}]}, "parent_tool_use_id": null}),
        ],
        &[],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    // Frames arrive in order, so once the text after the cancel shows, the cancelled request is gone.
    wait_for(&rx, |_| {
        session.with_thread(|t| {
            t.pending_requests().len() == 2
                && t.entries(&Lane::Main)
                    .iter()
                    .any(|e| format!("{:?}", e.kind).contains("after-cancel"))
        })
    });
    session
        .answer_questions(
            "cli-q",
            vec![term_thread::Answer {
                question: "Which color?".into(),
                answer: "blue".into(),
            }],
        )
        .unwrap();
    session
        .answer_plan(
            "cli-p",
            term_claude::PlanAnswer::Revise(Some("Add a test first".into())),
        )
        .unwrap();
    assert!(matches!(
        session.answer_plan("cli-gone", term_claude::PlanAnswer::Approve),
        Err(SessionError::UnknownRequest(id)) if id == "cli-gone"
    ));
    assert!(session.with_thread(|t| t.pending_requests().is_empty()));
    session.close_input();
    wait_for(&rx, |e| matches!(e, Event::Ended { .. }));

    let frames = written(&dir);
    let answer = |id: &str| {
        frames
            .iter()
            .find(|f| f["response"]["request_id"] == id)
            .map(|f| f["response"]["response"].clone())
            .unwrap_or_else(|| panic!("no answer to {id}"))
    };
    let chosen = answer("cli-q");
    assert_eq!(chosen["behavior"], "allow");
    assert_eq!(chosen["updatedInput"]["answers"]["Which color?"], "blue");
    assert_eq!(chosen["updatedInput"]["questions"], question["questions"]);
    let revised = answer("cli-p");
    assert_eq!(revised["behavior"], "deny");
    assert_eq!(revised["message"], "Add a test first");
}

#[test]
fn session_errors_say_what_the_user_can_do() {
    let texts = [
        SessionError::NotFound("/opt/claude".into()).to_string(),
        SessionError::Start("no pipes".into()).to_string(),
        SessionError::SignInRequired("Not logged in".into()).to_string(),
        SessionError::RewindRejected("drops a turn".into()).to_string(),
        SessionError::UnknownRequest("cli-9".into()).to_string(),
        SessionError::Closed.to_string(),
    ];
    assert_eq!(
        texts,
        [
            "/opt/claude not found",
            "claude did not start: no pipes",
            "sign in by running claude in a terminal: Not logged in",
            "the rewind was refused: drops a turn",
            "no request cli-9 waits for an answer",
            "the claude process has ended",
        ]
    );
}

#[test]
fn a_message_that_waited_for_the_start_is_shown_once_with_the_cli_id() {
    let dir = scratch("waited");
    let launch = stand_in(&dir, json!({"subscriptionType": "pro"}), &[]);
    let message = UserMessage {
        text: "Hi".into(),
        message_id: Some(format!("{}1", term_thread::WAITING_ID)),
        ..UserMessage::default()
    };
    let mut thread = Thread::new(ThreadId::new(), "claude", launch.cwd.clone());
    thread.apply(term_thread::Update::Put {
        lane: Lane::Main,
        key: message.waiting_key(),
        at: std::time::SystemTime::now(),
        entry: EntryKind::UserMessage(UserMessage {
            message_id: None,
            ..message.clone()
        }),
    });
    let (tx, _rx) = channel();
    let supervisor = Supervisor::new();
    let session = Session::start(&supervisor, &launch, thread, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    let uuid = session.send(message).unwrap();
    let ids: Vec<Option<String>> = session.with_thread(|t| {
        t.entries(&Lane::Main)
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::UserMessage(m) => Some(m.message_id.clone()),
                _ => None,
            })
            .collect()
    });
    assert_eq!(ids, vec![Some(uuid)]);
    session.close_input();
}

#[test]
fn a_subagents_permission_request_waits_at_the_end_of_the_main_timeline() {
    let dir = scratch("subagent-asks");
    let launch = stand_in(
        &dir,
        json!({"subscriptionType": "pro"}),
        &[
            json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "toolu_sub", "name": "Agent", "input": {"description": "look around", "subagent_type": "Explore", "prompt": "Look"}}]}, "parent_tool_use_id": null}),
            json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "tool_use", "id": "toolu_in", "name": "Bash", "input": {"command": "ls"}}]}, "parent_tool_use_id": "toolu_sub"}),
            json!({"type": "control_request", "request_id": "cli-1", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "toolu_in"}}),
        ],
    );
    let supervisor = Supervisor::new();
    let (session, rx) = start(&launch, &supervisor);
    wait_for(&rx, |_| session.with_thread(|t| !t.pending_requests().is_empty()));
    session.with_thread(|t| {
        let last = t.entries(&Lane::Main).last().map(|e| e.kind.clone());
        assert!(matches!(last, Some(EntryKind::PermissionRequest(_))), "{last:?}");
        let call = t.entry_by_key("toolu_in").map(|e| e.id);
        assert_eq!(
            call.and_then(|id| t.place(id)).map(|(lane, _)| lane.clone()),
            Some(Lane::Subagent("toolu_sub".into()))
        );
    });
    session.close_input();
}
