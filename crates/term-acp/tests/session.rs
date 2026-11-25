//! Sessions against the test agent in this crate: a whole turn with every kind of update, permissions, cancelling,
//! signing in, loading a session, modes and options, an agent that speaks another protocol version, and recording a
//! session for replay.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use term_base::testing::scratch;

use term_acp::test_agent::{COMMAND_OUTPUT, CRASH, SESSION};
use term_acp::{Choice, Event, Launch, Session, SessionError};
use term_process::Supervisor;
use term_thread::{
    Decision, EntryKind, Lane, Notice, RequestState, Status, Thread, ThreadId, TodoStatus, ToolOutput, ToolStatus,
    ToolView, UserMessage,
};

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);

impl Events {
    fn has(&self, wanted: impl Fn(&Event) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(wanted)
    }

    fn ends(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, Event::Ended { .. }))
            .count()
    }
}

fn launch(scenario: &str, cwd: &Path) -> Launch {
    let mut launch = Launch::new(
        "acp-test-agent",
        "Test Agent",
        env!("CARGO_BIN_EXE_acp-test-agent"),
        vec![scenario.into()],
        vec![("PATH".into(), "/usr/bin:/bin".into())],
        cwd,
    );
    launch.trusted = true;
    launch
}

fn start(launch: Launch) -> (Session, Arc<Events>, Supervisor) {
    let supervisor = Supervisor::new();
    let events = Arc::new(Events::default());
    let heard = events.clone();
    let thread = Thread::new(ThreadId::from("t"), launch.agent.clone(), launch.cwd.clone());
    let session = Session::start(&supervisor, launch, thread, move |e| heard.0.lock().unwrap().push(e)).unwrap();
    (session, events, supervisor)
}

fn wait(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "waited in vain: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn message(text: &str) -> UserMessage {
    UserMessage {
        text: text.into(),
        ..UserMessage::default()
    }
}

fn ready(scenario: &str, cwd: &Path) -> (Session, Arc<Events>, Supervisor) {
    let (session, events, supervisor) = start(launch(scenario, cwd));
    wait("a session", || session.is_ready());
    (session, events, supervisor)
}

fn answered_turn(choice: Choice, cwd: &Path) -> Thread {
    let (session, _, _) = ready("basic", cwd);
    session.send(message("list the files")).unwrap();
    wait("a permission request", || {
        session.snapshot().pending_requests().len() == 1
    });
    assert_eq!(session.snapshot().status(), Status::RequiresAction);
    let thread = session.snapshot();
    let request = match &thread.pending_requests()[0].kind {
        EntryKind::PermissionRequest(r) => r.request_id.clone(),
        other => panic!("{other:?}"),
    };
    session.answer(&request, choice).unwrap();
    wait("the end of the turn", || {
        let thread = session.snapshot();
        thread.status() == Status::Idle && thread.title().contains("Listing")
    });
    let thread = session.snapshot();
    session.stop(Duration::from_millis(500));
    thread
}

fn tool(thread: &Thread, id: &str) -> term_thread::ToolCall {
    thread
        .entries(&Lane::Main)
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolCall(call) if call.call_id == id => Some(call.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tool call {id}"))
}

#[test]
fn the_session_offers_what_the_agent_reported() {
    let dir = scratch("acp-session");
    let (session, _, _) = ready("basic", &dir);
    wait("the commands", || !session.capabilities().commands.is_empty());
    let caps = session.capabilities();
    assert_eq!(caps.protocol_version, Some(1));
    assert_eq!(caps.agent.as_ref().map(|a| a.0.as_str()), Some("acp-test-agent"));
    assert!(caps.load_session && caps.images && caps.embedded_context);
    assert_eq!(
        caps.modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["ask", "code"]
    );
    assert_eq!(caps.mode.as_deref(), Some("ask"));
    assert_eq!(caps.model().map(|m| m.current_value()), Some("fast".into()));
    assert_eq!(caps.commands[0].name, "review");
    assert_eq!(caps.commands[0].hint.as_deref(), Some("what to review"));
    assert_eq!(caps.unknown[0].0, "agentCapabilities._meta.test.vendor");
    assert_eq!(session.session_id().as_deref(), Some(SESSION));
    let thread = session.snapshot();
    assert_eq!(thread.session(), Some(SESSION));
    assert_eq!(thread.model(), Some("fast"));
    session.stop(Duration::from_millis(500));
}

#[test]
fn prompt_usage_reaches_the_thread_with_and_without_standard_usage() {
    let dir = scratch("acp-session");
    for scenario in ["usage-tokens", "usage-metadata", "usage-credits"] {
        let (session, _, _) = ready(scenario, &dir);
        for _ in 0..2 {
            session.send(message("usage please")).unwrap();
            wait("the usage response", || {
                let thread = session.snapshot();
                thread.status() == Status::Idle && thread.usage().reported_tokens.is_some()
            });
            let thread = session.snapshot();
            assert_eq!(thread.usage().reported_tokens.as_ref().unwrap().total, 1000);
            assert!(thread.usage().turns.is_empty(), "no assumed turn/session scope");
            assert!(!thread.usage().subscription, "token quota is not a plan allowance");
            if scenario == "usage-credits" {
                let cost = thread.usage().reported_cost.as_ref().unwrap();
                assert_eq!((cost.amount, cost.currency.as_str()), (2.5, "credits"));
                assert_eq!(thread.usage().context_used, Some(12000));
            } else {
                assert_eq!(thread.usage().context_used, None);
            }
        }
        session.stop(Duration::from_millis(500));
    }
}

#[test]
fn a_turn_fills_the_thread_with_every_kind_of_entry() {
    let dir = scratch("acp-session");
    let thread = answered_turn(Choice::Option(0), &dir);
    let kinds: Vec<&EntryKind> = thread.entries(&Lane::Main).iter().map(|e| &e.kind).collect();
    assert!(matches!(kinds[0], EntryKind::UserMessage(m) if m.text == "list the files"));
    assert!(matches!(kinds[1], EntryKind::Thought(t) if t.text == "Thinking about it." && !t.streaming));
    assert!(matches!(kinds[2], EntryKind::AgentMessage(m) if m.markdown == "Let me look." && !m.streaming));
    let ls = tool(&thread, "call-1");
    assert_eq!(ls.status, ToolStatus::Completed);
    assert!(matches!(&ls.view, ToolView::Command { command, .. } if command == "ls -la"));
    assert_eq!(ls.output, Some(ToolOutput::Text(COMMAND_OUTPUT.into())));
    assert!(ls.started.is_some() && ls.ended.is_some());
    let edit = tool(&thread, "call-2");
    let Some(ToolOutput::Diff { path, hunks }) = &edit.output else {
        panic!("{:?}", edit.output)
    };
    let edited = dir.canonicalize().unwrap().join("a.txt");
    assert_eq!(std::path::Path::new(path).canonicalize().unwrap(), edited);
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        term_acp::test_agent::EDITED.2
    );
    assert_eq!(hunks[0].lines, [" one", "-two", "+2"]);
    let permission = kinds.iter().find_map(|k| match k {
        EntryKind::PermissionRequest(r) => Some(r),
        _ => None,
    });
    let permission = permission.expect("a permission entry");
    assert_eq!(
        (permission.tool_name.as_str(), permission.title.as_deref()),
        ("ls -la", Some("ls -la"))
    );
    assert!(permission.input.is_null(), "an allowed request lets go of its input");
    assert_eq!(
        Some(&permission.state),
        Some(&RequestState::Decided(Decision::Allowed {
            always: false,
            edited: false
        }))
    );
    assert!(thread.todos().iter().all(|t| t.status == TodoStatus::Completed));
    assert_eq!(thread.usage().context_used, Some(12000));
    assert_eq!(thread.usage().context_size, Some(200000));
    assert_eq!(thread.usage().cost_usd, Some(0.0123));
    let last = kinds.last().unwrap();
    assert!(
        matches!(last, EntryKind::AgentMessage(m) if m.markdown == "Done; reading files was refused."),
        "the client refuses a file read it never offered: {last:?}"
    );
}

#[test]
fn a_refused_command_fails_its_tool_call() {
    let dir = scratch("acp-session");
    let thread = answered_turn(Choice::Option(2), &dir);
    assert_eq!(tool(&thread, "call-1").status, ToolStatus::Failed);
    let denied = thread.entries(&Lane::Main).iter().any(|e| {
        matches!(&e.kind, EntryKind::PermissionRequest(r)
            if r.state == RequestState::Decided(Decision::Denied { message: None }))
    });
    assert!(denied);
}

#[test]
fn cancelling_ends_the_turn_and_says_so() {
    let dir = scratch("acp-session");
    let (session, _, _) = ready("basic", &dir);
    session.send(message("go slow")).unwrap();
    wait("the first chunk", || {
        session
            .snapshot()
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == "Working on it"))
    });
    session.cancel();
    wait("the cancelled turn", || session.snapshot().status() == Status::Idle);
    let thread = session.snapshot();
    let last = &thread.entries(&Lane::Main).last().unwrap().kind;
    assert!(matches!(last, EntryKind::Notice(Notice::Interrupted)), "{last:?}");
    session.stop(Duration::from_millis(500));
}

#[test]
fn an_agent_that_wants_a_sign_in_opens_the_session_after_it() {
    let dir = scratch("acp-session");
    let (session, events, _) = start(launch("auth", &dir));
    wait("the request to sign in", || {
        events.has(|e| *e == Event::Failed(SessionError::SignIn))
    });
    assert!(!session.is_ready());
    let methods = session.capabilities().auth_methods;
    assert_eq!(methods[0].id, "browser");
    assert_eq!(methods[0].terminal, None);
    session.authenticate("browser").unwrap();
    wait("a session after signing in", || session.is_ready());
    session.stop(Duration::from_millis(500));
}

#[test]
fn an_agent_of_another_protocol_version_is_refused() {
    let dir = scratch("acp-session");
    let (session, events, _) = start(launch("v2", &dir));
    wait("the refusal", || {
        events.has(|e| *e == Event::Failed(SessionError::Version(2)))
    });
    assert!(!session.is_ready());
    session.stop(Duration::from_millis(500));
}

#[test]
fn a_loaded_session_replays_its_conversation() {
    let dir = scratch("acp-session");
    let mut launch = launch("basic", &dir);
    launch.resume = Some("earlier-session".into());
    let (session, _, _) = start(launch);
    wait("the replayed answer", || {
        session
            .snapshot()
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == "Earlier answer" && !m.streaming))
    });
    let thread = session.snapshot();
    assert!(matches!(&thread.entries(&Lane::Main)[0].kind, EntryKind::UserMessage(m) if m.text == "Earlier question"));
    assert_eq!(thread.session(), Some("earlier-session"));
    assert_eq!(thread.model(), Some("deep"));
    session.stop(Duration::from_millis(500));
}

#[test]
fn a_session_continued_on_a_filled_thread_replaces_its_conversation() {
    let dir = scratch("acp-session");
    let (first, _, _) = ready("basic", &dir);
    first.send(message("go slow")).unwrap();
    wait("the first answer", || first.snapshot().entries(&Lane::Main).len() >= 2);
    first.cancel();
    wait("the end of the turn", || first.snapshot().status() == Status::Idle);
    let thread = first.snapshot();
    first.stop(Duration::from_millis(500));
    let mut launch = launch("basic", &dir);
    launch.resume = Some(SESSION.into());
    let supervisor = Supervisor::new();
    let second = Session::start(&supervisor, launch, thread, |_| {}).unwrap();
    wait("the replayed conversation", || second.is_ready());
    let texts: Vec<String> = second
        .snapshot()
        .entries(&Lane::Main)
        .iter()
        .map(|e| match &e.kind {
            EntryKind::UserMessage(m) => format!("user {}", m.text),
            EntryKind::AgentMessage(m) => format!("agent {}", m.markdown),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(texts, ["user Earlier question", "agent Earlier answer"]);
    second.stop(Duration::from_millis(500));
}

#[test]
fn modes_and_options_change_on_request() {
    let dir = scratch("acp-session");
    let (session, _, _) = ready("basic", &dir);
    session.set_mode("code").unwrap();
    wait("the new mode", || {
        session.capabilities().mode.as_deref() == Some("code")
    });
    session.set_option("model", "deep").unwrap();
    wait("the new model", || {
        session
            .capabilities()
            .model()
            .is_some_and(|m| m.current_value() == "deep")
    });
    assert_eq!(session.snapshot().model(), Some("deep"));
    session.stop(Duration::from_millis(500));
}

#[test]
fn stopping_ends_the_process_and_says_so() {
    let dir = scratch("acp-session");
    let (session, events, supervisor) = ready("basic", &dir);
    session.stop(Duration::from_millis(500));
    wait("the end", || {
        session.is_ended() && events.has(|e| matches!(e, Event::Ended { .. }))
    });
    wait("no agent process left", || supervisor.running().is_empty());
    assert_eq!(session.send(message("hello")), Err(SessionError::Closed));
}

#[test]
fn a_session_through_the_recorder_works_and_leaves_both_sides_in_the_recording() {
    let dir = scratch("acp-session");
    let file = dir.join("recorded.ndjson");
    let _ = std::fs::remove_file(&file);
    let mut recorded = launch("basic", &dir);
    let mut args = vec![
        file.to_string_lossy().into_owned(),
        recorded.program.to_string_lossy().into_owned(),
    ];
    args.append(&mut recorded.args);
    recorded.program = PathBuf::from(env!("CARGO_BIN_EXE_acp-record"));
    recorded.args = args;
    let (session, _, _) = start(recorded);
    wait("a session", || session.is_ready());
    session.stop(Duration::from_millis(500));
    // The recorder may still be writing its last line when the session has stopped.
    let recorded = || -> Option<Vec<serde_json::Value>> {
        let text = std::fs::read_to_string(&file).ok()?;
        text.lines().map(|line| serde_json::from_str(line).ok()).collect()
    };
    wait("a whole recording", || recorded().is_some());
    let lines = recorded().unwrap();
    assert_eq!(lines[0]["to"], "agent");
    assert_eq!(lines[0]["message"]["method"], "initialize");
    assert!(
        lines
            .iter()
            .any(|l| l["to"] == "client" && l["message"]["result"]["sessionId"] == SESSION)
    );
}

fn permission(thread: &Thread) -> term_thread::PermissionRequest {
    thread
        .entries(&Lane::Main)
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::PermissionRequest(r) => Some(r.clone()),
            _ => None,
        })
        .expect("a permission request")
}

#[test]
fn allowing_always_and_dismissing_are_told_apart() {
    let dir = scratch("acp-session");
    let always = answered_turn(Choice::Option(1), &dir);
    assert_eq!(
        permission(&always).state,
        RequestState::Decided(Decision::Allowed {
            always: true,
            edited: false
        })
    );
    assert_eq!(tool(&always, "call-1").status, ToolStatus::Completed);
    let dismissed = answered_turn(Choice::Dismiss, &dir);
    assert_eq!(
        permission(&dismissed).state,
        RequestState::Decided(Decision::Denied { message: None })
    );
    assert_eq!(tool(&dismissed, "call-1").status, ToolStatus::Failed);
}

#[test]
fn a_sink_that_panics_costs_its_events_not_the_connection_and_unknown_requests_are_named() {
    let dir = scratch("acp-panicking-sink");
    let supervisor = Supervisor::new();
    let launch = launch("basic", &dir);
    let thread = Thread::new(ThreadId::from("t"), launch.agent.clone(), launch.cwd.clone());
    let session = Session::start(&supervisor, launch, thread, |_| panic!("a sink that panics")).unwrap();
    wait("a session", || session.is_ready());
    session.send(message("list the files")).unwrap();
    wait("a permission request", || {
        session.snapshot().pending_requests().len() == 1
    });
    assert_eq!(
        session.answer("nope", Choice::Dismiss),
        Err(SessionError::UnknownRequest("nope".into()))
    );
    let request = permission(&session.snapshot()).request_id;
    session.answer(&request, Choice::Option(0)).unwrap();
    wait("the end of the turn", || {
        let thread = session.snapshot();
        thread.status() == Status::Idle && thread.title().contains("Listing")
    });
    assert_eq!(tool(&session.snapshot(), "call-1").status, ToolStatus::Completed);
    session.stop(Duration::from_millis(500));
}

#[test]
fn cancelling_while_the_agent_asks_withdraws_the_request() {
    let dir = scratch("acp-session");
    let (session, _, _) = ready("basic", &dir);
    session.send(message("list the files")).unwrap();
    wait("a permission request", || {
        session.with_thread(|t| t.pending_requests().len() == 1)
    });
    let request = permission(&session.snapshot()).request_id;
    session.cancel();
    wait("the cancelled turn", || session.snapshot().status() == Status::Idle);
    assert_eq!(permission(&session.snapshot()).state, RequestState::Withdrawn);
    assert_eq!(
        session.answer(&request, Choice::Option(0)),
        Err(SessionError::UnknownRequest(request.clone())),
        "a withdrawn request takes no answer"
    );
    session.stop(Duration::from_millis(500));
}

#[test]
fn an_agent_that_dies_at_start_is_reported_and_its_last_words_come_with_the_end() {
    let dir = scratch("acp-session");
    let mut launch = launch("basic", &dir);
    launch.program = "/bin/sh".into();
    launch.args = vec!["-c".into(), "echo 'no API key set' >&2; exit 3".into()];
    let (session, events, _) = start(launch);
    wait("the failure", || {
        events.has(|e| matches!(e, Event::Failed(SessionError::Agent(_))))
    });
    assert!(!session.is_ready());
    session.cancel();
    assert_eq!(session.send(message("hello")), Err(SessionError::Closed));
    assert_eq!(session.set_mode("code"), Err(SessionError::Closed));
    assert_eq!(session.set_flag("fast", true), Err(SessionError::Closed));
    session.stop(Duration::from_millis(500));
    wait("the end", || events.has(|e| matches!(e, Event::Ended { .. })));
    assert!(events.has(|e| matches!(e, Event::Ended { stderr } if stderr.trim() == "no API key set")));
    assert_eq!(session.authenticate("browser"), Err(SessionError::Closed));
}

#[test]
fn a_missing_program_is_reported_before_anything_starts_and_errors_read_plainly() {
    let dir = scratch("acp-session");
    let mut launch = launch("basic", &dir);
    launch.program = "/nonexistent/agent".into();
    let thread = Thread::new(ThreadId::from("t"), "x", "/tmp");
    let result = Session::start(&Supervisor::new(), launch, thread, |_| {});
    assert!(matches!(result, Err(SessionError::NotFound(ref p)) if p == std::path::Path::new("/nonexistent/agent")));
    let texts = [
        SessionError::NotFound("/opt/agent".into()),
        SessionError::Start("no pipes".into()),
        SessionError::SignIn,
        SessionError::Version(2),
        SessionError::Agent("model overloaded".into()),
        SessionError::Closed,
    ]
    .map(|e| e.to_string());
    assert_eq!(
        texts,
        [
            "/opt/agent is not there",
            "the agent did not start: no pipes",
            "the agent asks for a sign-in",
            "the agent speaks protocol version 2",
            "model overloaded",
            "the session has ended"
        ]
    );
}

/// Drops `session` and waits until nothing of it holds the sink any more, after which no event can arrive.
fn let_go(session: Session, events: &Arc<Events>) {
    drop(session);
    wait("the session lets go of its sink", || Arc::strong_count(events) == 1);
}

/// Waits until the session ended on its own, then checks that it stays ended once, also after `stop`; the thread as
/// it was left.
fn ended_once(session: Session, events: &Arc<Events>, supervisor: &Supervisor) -> Thread {
    wait("the end", || session.is_ended() && events.ends() == 1);
    wait("no agent process left", || supervisor.running().is_empty());
    assert_eq!(session.send(message("hello")), Err(SessionError::Closed));
    session.stop(Duration::from_millis(100));
    let thread = session.snapshot();
    let_go(session, events);
    assert_eq!(events.ends(), 1, "stop after the end ends nothing more");
    thread
}

#[test]
fn an_agent_that_exits_mid_turn_ends_the_session_with_its_reason() {
    let dir = scratch("acp-session");
    let (session, events, supervisor) = ready("basic", &dir);
    session.send(message("crash now")).unwrap();
    let thread = ended_once(session, &events, &supervisor);
    assert!(events.has(|e| matches!(e, Event::Ended { stderr } if stderr.trim() == CRASH)));
    assert_eq!(thread.status(), Status::Idle);
    let errors: Vec<&Notice> = thread
        .entries(&Lane::Main)
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Notice(notice @ Notice::Error { .. }) => Some(notice),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        matches!(errors[0], Notice::Error { message, .. } if message == CRASH),
        "{errors:?}"
    );
}

#[test]
fn an_agent_whose_output_outlives_it_still_ends_the_session() {
    let dir = scratch("acp-session");
    let mut launch = launch("basic", &dir);
    let script = format!("sleep 10 & exec '{}' basic", launch.program.display());
    launch.program = "/bin/sh".into();
    launch.args = vec!["-c".into(), script];
    let (session, events, supervisor) = start(launch);
    wait("a session", || session.is_ready());
    let crashed = Instant::now();
    session.send(message("crash now")).unwrap();
    wait("the end", || session.is_ended());
    assert!(
        crashed.elapsed() < Duration::from_secs(5),
        "the end waited for the open output"
    );
    assert_eq!(ended_once(session, &events, &supervisor).status(), Status::Idle);
    assert!(events.has(|e| matches!(e, Event::Ended { stderr } if stderr.trim() == CRASH)));
}

#[test]
fn an_agent_that_exits_while_idle_ends_the_session() {
    let dir = scratch("acp-session");
    let (session, events, supervisor) = ready("basic", &dir);
    session.send(message("quit")).unwrap();
    let thread = ended_once(session, &events, &supervisor);
    assert_eq!(thread.status(), Status::Idle);
    assert!(
        !thread
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::Notice(Notice::Error { .. }))),
        "a clean exit after the turn is no error"
    );
}

#[test]
fn stopping_a_live_agent_ends_it_once() {
    let dir = scratch("acp-session");
    let (session, events, supervisor) = ready("basic", &dir);
    session.stop(Duration::from_millis(500));
    session.stop(Duration::from_millis(500));
    wait("the end", || session.is_ended() && events.ends() == 1);
    wait("no agent process left", || supervisor.running().is_empty());
    let_go(session, &events);
    assert_eq!(events.ends(), 1);
    assert!(!events.has(|e| matches!(e, Event::Failed(_))), "a stop is no failure");
}

#[test]
fn an_agent_that_cannot_skip_the_projects_configuration_does_not_start_untrusted() {
    let dir = scratch("acp-untrusted");
    let mut untrusted = launch("basic", &dir);
    untrusted.trusted = false;
    let thread = Thread::new(ThreadId::from("t"), untrusted.agent.clone(), untrusted.cwd.clone());
    let refused = Session::start(&Supervisor::new(), untrusted, thread, |_| {}).err();
    assert_eq!(refused, Some(SessionError::Untrusted("Test Agent".into())));
}
