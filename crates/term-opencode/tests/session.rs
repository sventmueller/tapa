//! Sessions against the stand-in server of this crate: the handshake in order with basic auth, a whole turn with
//! every kind of event, permissions, forms, cancelling, steering, a crash, a failed turn, resuming, the version gate,
//! the settings and the management calls; every request the client sends is checked against the schema.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use term_opencode::test_server::{COMMAND_OUTPUT, CRASH, FORK, FORM, GAP_ANSWER, PERMISSION, SESSION};
use term_opencode::{
    Choice, CredentialKind, Event, Launch, McpConfig, McpStatus, ServerError, Session, SessionError, SessionQuery,
    TESTED,
};
use term_process::Supervisor;
use term_thread::{
    Answer, Decision, EntryKind, ErrorKind, Lane, LimitStatus, Notice, RequestState, Status, Thread, ThreadId,
    TodoStatus, ToolOutput, ToolStatus, ToolView, UserMessage,
};

#[derive(Default)]
struct Events(Mutex<Vec<Event>>);

impl Events {
    fn has(&self, wanted: impl Fn(&Event) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(wanted)
    }
}

struct Run {
    session: Session,
    events: Arc<Events>,
    log: PathBuf,
    supervisor: Supervisor,
}

impl Run {
    /// What the client sent, request by request.
    fn sent(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn routes(&self) -> Vec<String> {
        self.sent()
            .iter()
            .map(|r| format!("{} {}", r["method"].as_str().unwrap(), r["path"].as_str().unwrap()))
            .collect()
    }

    fn last(&self, method: &str, path: &str) -> Value {
        self.sent()
            .into_iter()
            .rev()
            .find(|r| r["method"] == method && r["path"] == path)
            .unwrap_or(Value::Null)
    }

    /// Every request was authorized and is an operation of the schema with a body it describes.
    fn conforms(&self) {
        let spec = support::schema::Spec::load();
        for request in self.sent() {
            assert_eq!(request["authorized"], true, "{request}");
            let query = request["query"].as_object().cloned().unwrap_or_default();
            if let Err(error) = spec.check_request(
                request["method"].as_str().unwrap(),
                request["path"].as_str().unwrap(),
                &query,
                &request["body"],
            ) {
                panic!("{error}");
            }
        }
    }
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("term-opencode-session-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn launch(scenario: &str, name: &str) -> (Launch, PathBuf) {
    let dir = folder(name);
    let log = dir.join("sent.ndjson");
    let _ = std::fs::remove_file(&log);
    let env = vec![
        ("PATH".into(), "/usr/bin:/bin".into()),
        ("OPENCODE_PASSWORD".into(), "from the shell".into()),
        ("OPENCODE_TEST_SCENARIO".into(), scenario.into()),
        ("OPENCODE_TEST_LOG".into(), log.to_string_lossy().into_owned()),
    ];
    (Launch::new(env!("CARGO_BIN_EXE_opencode-test-server"), env, dir), log)
}

fn start(launch: Launch, log: PathBuf) -> Run {
    let supervisor = Supervisor::new();
    let events = Arc::new(Events::default());
    let heard = events.clone();
    let thread = Thread::new(ThreadId::from("t"), term_opencode::AGENT, launch.cwd.clone());
    let session = Session::start(&supervisor, launch, thread, move |e| heard.0.lock().unwrap().push(e)).unwrap();
    Run {
        session,
        events,
        log,
        supervisor,
    }
}

fn wait(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "waited in vain: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn ready(scenario: &str, name: &str) -> Run {
    let (launch, log) = launch(scenario, name);
    let run = start(launch, log);
    wait("an open session", || run.session.is_ready());
    run
}

fn message(text: &str) -> UserMessage {
    UserMessage {
        text: text.into(),
        ..UserMessage::default()
    }
}

fn kinds(session: &Session) -> Vec<EntryKind> {
    session.with_thread(|t| t.entries(&Lane::Main).iter().map(|e| e.kind.clone()).collect())
}

fn pending_request(session: &Session) -> Option<String> {
    session.with_thread(|t| {
        t.entries(&Lane::Main).iter().find_map(|e| match &e.kind {
            EntryKind::PermissionRequest(p) if p.state == RequestState::Pending => Some(p.request_id.clone()),
            EntryKind::Question(q) if q.state == RequestState::Pending => q.request_id.clone(),
            _ => None,
        })
    })
}

fn await_request(session: &Session) -> String {
    wait("a request", || pending_request(session).is_some());
    pending_request(session).unwrap()
}

fn idle(session: &Session) -> bool {
    session.with_thread(|t| t.status() == Status::Idle)
}

fn card(session: &Session, name: &str) -> term_thread::ToolCall {
    kinds(session)
        .into_iter()
        .find_map(|k| match k {
            EntryKind::ToolCall(c) if c.name == name => Some(c),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {name} card"))
}

#[test]
fn the_handshake_runs_in_order_with_the_servers_own_password() {
    let run = ready("basic", "handshake");
    assert_eq!(
        run.routes()[..7],
        [
            "GET /api/info",
            "GET /api/event",
            "GET /api/agent",
            "GET /api/model",
            "GET /api/model/default",
            "GET /api/provider",
            "POST /api/session"
        ],
        "{:?}",
        run.routes()
    );
    let created = run.last("POST", "/api/session");
    let caps = run.session.capabilities();
    assert_eq!(
        created["body"]["location"]["directory"],
        folder("handshake").to_string_lossy().as_ref()
    );
    assert_eq!(
        run.sent()[2]["query"]["location[directory]"],
        created["body"]["location"]["directory"]
    );
    assert_eq!(run.session.session_id().as_deref(), Some(SESSION));
    assert_eq!(caps.version, Some(TESTED));
    assert_eq!(caps.server_version.as_deref(), Some("2.2.6"));
    let modes: Vec<&str> = caps.modes().iter().map(|a| a.id.as_str()).collect();
    assert_eq!(modes, ["build", "plan"]);
    assert_eq!(caps.models.len(), 2);
    assert_eq!(caps.variants(), ["high", "max"]);
    assert!(caps.images() && caps.signed_in());
    run.session.with_thread(|t| {
        assert_eq!(t.session(), Some(SESSION));
        assert_eq!(t.model(), Some("anthropic/claude-sonnet-4"));
        assert_eq!(t.mode(), Some("build"));
        assert_eq!(t.status(), Status::Idle);
    });
    let labels: Vec<String> = run.supervisor.running().into_iter().map(|p| p.label).collect();
    assert!(labels.contains(&"OpenCode".to_string()), "{labels:?}");
    assert!(!run.events.has(|e| matches!(e, Event::Failed(_))));
    run.conforms();
}

#[test]
fn a_whole_turn_fills_the_thread_and_the_permission_goes_out_as_always() {
    let run = ready("basic", "turn");
    run.session.send(message("list the files")).unwrap();
    let asked = await_request(&run.session);
    assert_eq!(asked, PERMISSION);
    let now = kinds(&run.session);
    assert!(matches!(&now[0], EntryKind::UserMessage(m) if m.text == "list the files"));
    assert!(matches!(&now[1], EntryKind::Thought(t) if t.text == "Reading the files." && !t.streaming));
    assert!(matches!(&now[2], EntryKind::AgentMessage(m) if m.markdown == "Let me look." && !m.streaming));
    let EntryKind::PermissionRequest(permission) = now.last().unwrap() else {
        panic!("{now:?}")
    };
    assert_eq!(permission.options.len(), 3, "once, always, reject");
    assert_eq!(permission.tool_name, "Bash");
    assert_eq!(permission.title.as_deref(), Some("ls"));
    assert_eq!(card(&run.session, "Bash").status, ToolStatus::Pending);
    assert_eq!(run.session.with_thread(Thread::status), Status::RequiresAction);
    run.session.answer(&asked, Choice::Option(1)).unwrap();
    wait("the end of the turn", || {
        idle(&run.session) && run.session.with_thread(|t| t.title() == "Listing files")
    });
    assert_eq!(
        run.last("POST", &format!("/api/session/{SESSION}/permission/{PERMISSION}/reply"))["body"],
        json!({"decision": "always"})
    );
    let shell = card(&run.session, "Bash");
    assert_eq!(shell.status, ToolStatus::Completed);
    assert!(
        matches!(&shell.view, ToolView::Command { command, description: Some(d), .. } if command == "ls" && d == "List files")
    );
    assert!(matches!(&shell.output, Some(ToolOutput::Terminal { stdout, .. }) if stdout == COMMAND_OUTPUT));
    let edit = card(&run.session, "Edit");
    assert!(matches!(&edit.output, Some(ToolOutput::Diff { hunks, .. }) if hunks[0].lines == [" one", "-two", "+2"]));
    assert!(matches!(&card(&run.session, "Read").view, ToolView::Read { path, .. } if path == "a.txt"));
    let entries = kinds(&run.session);
    assert!(entries.iter().any(|k| matches!(k, EntryKind::PermissionRequest(p)
        if p.state == RequestState::Decided(Decision::Allowed { always: true, edited: false }))));
    assert!(matches!(entries.last(), Some(EntryKind::AgentMessage(m)) if m.markdown == "Done."));
    run.session.with_thread(|t| {
        let states: Vec<TodoStatus> = t.todos().iter().map(|todo| todo.status).collect();
        assert_eq!(states, [TodoStatus::InProgress, TodoStatus::Pending]);
        let usage = t.usage();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.cache_read_tokens),
            (1200, 150, 11000)
        );
        assert_eq!(usage.cost_usd, Some(0.015));
        assert_eq!((usage.context_used, usage.context_size), (Some(6230), Some(200_000)));
        assert_eq!(usage.turns.len(), 1);
        assert!(usage.turns[0].duration.is_some());
    });
    run.conforms();
}

#[test]
fn a_rejected_command_fails_its_card() {
    let run = ready("basic", "reject");
    run.session.send(message("list the files")).unwrap();
    let asked = await_request(&run.session);
    run.session.answer(&asked, Choice::Option(2)).unwrap();
    wait("the end of the turn", || {
        idle(&run.session) && !kinds(&run.session).is_empty()
    });
    wait(
        "the last answer",
        || matches!(kinds(&run.session).last(), Some(EntryKind::AgentMessage(m)) if m.markdown == "Done."),
    );
    let shell = card(&run.session, "Bash");
    assert_eq!(shell.status, ToolStatus::Failed);
    assert_eq!(shell.output, Some(ToolOutput::Text("The user rejected it".into())));
    assert_eq!(
        run.last("POST", &format!("/api/session/{SESSION}/permission/{PERMISSION}/reply"))["body"]["decision"],
        "reject"
    );
    assert_eq!(
        run.session.answer("per_unknown", Choice::Dismiss),
        Err(SessionError::UnknownRequest("per_unknown".into()))
    );
}

#[test]
fn cancelling_interrupts_and_a_message_meanwhile_steers_the_turn() {
    let run = ready("basic", "cancel");
    run.session.send(message("slow please")).unwrap();
    wait("the streaming answer", || {
        kinds(&run.session)
            .iter()
            .any(|k| matches!(k, EntryKind::AgentMessage(m) if m.markdown == "Working"))
    });
    run.session.send(message("and one more thing")).unwrap();
    wait("the steering prompt", || {
        run.sent()
            .iter()
            .filter(|r| r["path"] == format!("/api/session/{SESSION}/prompt"))
            .count()
            == 2
    });
    assert_eq!(
        run.last("POST", &format!("/api/session/{SESSION}/prompt"))["body"]["delivery"],
        "steer"
    );
    assert!(
        kinds(&run.session)
            .iter()
            .any(|k| matches!(k, EntryKind::UserMessage(m) if m.text == "and one more thing"))
    );
    run.session.cancel();
    wait("the stopped turn", || idle(&run.session));
    assert!(run.routes().contains(&format!("POST /api/session/{SESSION}/interrupt")));
    assert!(
        kinds(&run.session)
            .iter()
            .any(|k| matches!(k, EntryKind::Notice(Notice::Interrupted)))
    );
    run.conforms();
}

#[test]
fn a_form_is_answered_by_its_fields_or_cancelled() {
    let run = ready("basic", "form");
    run.session.send(message("ask me")).unwrap();
    let asked = await_request(&run.session);
    assert_eq!(asked, FORM);
    let Some(EntryKind::Question(question)) = kinds(&run.session)
        .into_iter()
        .find(|k| matches!(k, EntryKind::Question(_)))
    else {
        panic!()
    };
    assert_eq!(question.items[0].question, "Which colour?");
    assert_eq!(question.items[0].options.len(), 2);
    assert_eq!(question.tool_call, "tool:msg_a1:call_q");
    assert_eq!(card(&run.session, "AskUserQuestion").call_id, "call_q");
    assert_eq!(
        run.session.answer(&asked, Choice::Option(0)),
        Err(SessionError::UnknownRequest(FORM.into())),
        "a form takes answers, not options"
    );
    let answers = vec![Answer {
        question: "Which colour?".into(),
        answer: "Red".into(),
    }];
    run.session.answer_questions(&asked, answers).unwrap();
    wait(
        "the answer",
        || matches!(kinds(&run.session).last(), Some(EntryKind::AgentMessage(m)) if m.markdown == "You chose red."),
    );
    assert_eq!(
        run.last("POST", &format!("/api/session/{SESSION}/form/{FORM}/reply"))["body"],
        json!({"answer": {"color": "red"}})
    );
    run.session.send(message("ask again")).unwrap();
    wait("the second form", || pending_request(&run.session).is_some());
    run.session.answer(FORM, Choice::Dismiss).unwrap();
    wait("the end", || idle(&run.session));
    assert!(
        run.routes()
            .contains(&format!("DELETE /api/session/{SESSION}/form/{FORM}"))
    );
    run.conforms();
}

#[test]
fn a_gap_in_the_event_stream_is_reported_and_reading_the_session_again_ends_the_turn_it_swallowed() {
    let run = ready("basic", "gap");
    run.session.send(message("mind the gap")).unwrap();
    wait("the gap", || run.events.has(|e| *e == Event::StreamGap));
    wait("the stream again", || run.events.has(|e| *e == Event::StreamResumed));
    assert!(!idle(&run.session), "the end of the turn came while no stream was open");
    run.session.resync().wait().unwrap();
    assert!(idle(&run.session));
    let kinds = kinds(&run.session);
    assert!(
        kinds
            .iter()
            .any(|k| matches!(k, EntryKind::AgentMessage(m) if m.markdown == GAP_ANSWER)),
        "{kinds:?}"
    );
    assert!(
        !kinds
            .iter()
            .any(|k| matches!(k, EntryKind::Notice(Notice::Error { .. })))
    );
    assert_eq!(
        run.session.with_thread(|t| t
            .entries(&Lane::Main)
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::UserMessage(_)))
            .count()),
        1
    );
    assert_eq!(run.session.with_thread(|t| t.usage().cost_usd), Some(0.02));
    assert_eq!(run.session.with_thread(|t| t.usage().output_tokens), 40);
    assert!(run.routes().contains(&format!("GET /api/session/{SESSION}/message")));
    run.conforms();
}

#[test]
fn a_crash_ends_the_session_with_its_last_words_and_a_failed_turn_says_why() {
    let run = ready("basic", "crash");
    run.session.send(message("fail now")).unwrap();
    wait("the failed turn", || {
        idle(&run.session)
            && kinds(&run.session).iter().any(|k| {
                matches!(
                    k,
                    EntryKind::Notice(Notice::Error {
                        kind: ErrorKind::Auth,
                        ..
                    })
                )
            })
    });
    run.session.send(message("crash please")).unwrap();
    wait("the end", || run.session.is_ended());
    wait("the ended event", || {
        run.events.has(|e| matches!(e, Event::Ended { .. }))
    });
    assert!(
        run.events
            .has(|e| matches!(e, Event::Ended { stderr } if stderr.contains(CRASH)))
    );
    assert!(kinds(&run.session).iter().any(|k| matches!(k,
        EntryKind::Notice(Notice::Error { kind: ErrorKind::Process, message }) if message.contains(CRASH))));
    assert!(idle(&run.session));
    assert_eq!(run.session.send(message("hello?")), Err(SessionError::Closed));
}

#[test]
fn resuming_shows_the_history_and_the_permission_that_still_waits() {
    let (mut launch, log) = launch("basic", "resume");
    launch.resume = Some("ses_old".into());
    let run = start(launch, log);
    wait("an open session", || run.session.is_ready());
    wait("the waiting permission", || pending_request(&run.session).is_some());
    let entries = kinds(&run.session);
    assert!(matches!(&entries[0], EntryKind::UserMessage(m) if m.text == "Earlier question"));
    assert!(
        matches!(&entries[1], EntryKind::Thought(t) if t.text == "Thinking back." && t.duration == Some(Duration::from_millis(500)))
    );
    assert!(matches!(&entries[2], EntryKind::ToolCall(c) if c.name == "Bash"
        && matches!(&c.output, Some(ToolOutput::Terminal { stdout, .. }) if stdout == "/work")));
    assert!(matches!(&entries[3], EntryKind::AgentMessage(m) if m.markdown == "Earlier answer"));
    assert!(matches!(&entries[4], EntryKind::Notice(Notice::Interrupted)));
    let waiting: Vec<String> = entries
        .iter()
        .filter_map(|k| match k {
            EntryKind::PermissionRequest(p) => Some(p.request_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(waiting, [PERMISSION], "another session's permission is left out");
    run.session.with_thread(|t| {
        assert_eq!(t.title(), "Earlier");
        assert_eq!(
            (t.mode(), t.model(), t.effort()),
            (Some("plan"), Some("openai/gpt-5"), Some("high"))
        );
    });
    let pages = run
        .sent()
        .into_iter()
        .filter(|r| r["path"] == "/api/session/ses_old/message")
        .count();
    assert_eq!(pages, 2);
    run.session.answer(PERMISSION, Choice::Option(0)).unwrap();
    wait("the reply", || {
        run.last("POST", &format!("/api/session/ses_old/permission/{PERMISSION}/reply"))["body"]
            == json!({"decision": "once"})
    });
    run.conforms();
}

fn failure(scenario: &str, name: &str, resume: Option<&str>) -> Run {
    let (mut launch, log) = launch(scenario, name);
    launch.resume = resume.map(str::to_string);
    let run = start(launch, log);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !run.events.has(|e| matches!(e, Event::Failed(_))) {
        assert!(
            Instant::now() < deadline,
            "{scenario}: no failure in {:?}, sent {:?}",
            run.events.0.lock().unwrap(),
            run.routes()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    run
}

#[test]
fn an_old_release_a_missing_session_no_model_and_a_wrong_password_are_reported() {
    let old = failure("old", "old", None);
    assert!(
        old.events
            .has(|e| matches!(e, Event::Failed(SessionError::Unsupported { found: Some(v), .. }) if v.minor == 1))
    );
    wait("the end of an old release", || old.session.is_ended());
    let missing = failure("basic", "missing", Some("ses_missing"));
    assert!(
        missing
            .events
            .has(|e| matches!(e, Event::Failed(SessionError::Server(ServerError { status: 404, .. }))))
    );
    let nomodel = failure("nomodel", "nomodel", None);
    assert!(nomodel.events.has(|e| matches!(e, Event::Failed(SessionError::SignIn))));
    assert!(nomodel.session.is_ready(), "the session opens all the same");
    let locked = failure("locked", "locked", None);
    assert!(locked.events.has(|e| matches!(e,
        Event::Failed(SessionError::Server(ServerError { status: 401, tag: Some(tag), .. })) if tag == "UnauthorizedError")));
    assert!(locked.sent().iter().all(|r| r["authorized"] == false));
}

#[test]
fn settings_go_out_as_agent_and_model_and_come_back_as_events() {
    let run = ready("basic", "settings");
    run.session.set_mode("plan").unwrap();
    wait("the agent", || {
        run.last("POST", &format!("/api/session/{SESSION}/agent"))["body"] == json!({"agent": "plan"})
    });
    assert_eq!(
        run.session.with_thread(|t| t.mode().map(str::to_string)),
        Some("plan".into())
    );
    run.session.set_option("model", "openai/gpt-5").unwrap();
    wait("the model", || {
        run.last("POST", &format!("/api/session/{SESSION}/model"))["body"]
            == json!({"model": {"providerID": "openai", "id": "gpt-5"}})
    });
    run.session.set_option("effort", "high").unwrap();
    wait("the variant", || {
        run.last("POST", &format!("/api/session/{SESSION}/model"))["body"]["model"]["variant"] == "high"
    });
    wait("the selected model", || {
        run.session.with_thread(|t| t.effort() == Some("high"))
    });
    assert_eq!(run.session.capabilities().variant(), Some("high"));
    assert_eq!(
        run.session.set_mode("general"),
        Err(SessionError::UnknownOption("general".into()))
    );
    assert_eq!(
        run.session.set_option("model", "nobody/none"),
        Err(SessionError::UnknownOption("nobody/none".into()))
    );
    assert_eq!(
        run.session.set_option("effort", "max"),
        Err(SessionError::UnknownOption("max".into()))
    );
    assert_eq!(
        run.session.set_option("speed", "x"),
        Err(SessionError::UnknownOption("speed".into()))
    );
    run.conforms();
}

#[test]
fn the_management_calls_answer_in_their_types() {
    let run = ready("basic", "manage");
    let s = &run.session;
    assert_eq!(s.info().wait().unwrap().version, "2.2.6");
    assert_eq!(s.models().wait().unwrap().len(), 2);
    assert_eq!(s.agents().wait().unwrap().len(), 4);
    assert_eq!(s.providers().wait().unwrap()[0].id, "anthropic");
    let servers = s.mcp_servers().wait().unwrap();
    assert_eq!(servers[0].status, McpStatus::NeedsAuth("Sign in to docs".into()));
    let config = McpConfig::Remote {
        url: "https://mcp.example.test".into(),
        headers: vec![("Authorization".into(), "Bearer x".into())],
        disabled: false,
    };
    s.add_mcp("docs", &config).wait().unwrap();
    s.connect_mcp("docs").wait().unwrap();
    s.disconnect_mcp("docs").wait().unwrap();
    s.remove_mcp("docs").wait().unwrap();
    let query = SessionQuery {
        limit: Some(10),
        roots: true,
        ..SessionQuery::default()
    };
    assert_eq!(s.sessions(&query).wait().unwrap().sessions[0].id, SESSION);
    let fork = s.fork(Some("msg_a2")).wait().unwrap();
    assert_eq!((fork.id.as_str(), fork.parent.as_deref()), (FORK, Some(SESSION)));
    assert_eq!(s.export(true).wait().unwrap()["data"]["info"]["id"], SESSION);
    let revert = s.revert_stage("msg_a1", true).wait().unwrap();
    assert_eq!((revert.message_id.as_str(), revert.files.len()), ("msg_a1", 1));
    s.revert_commit().wait().unwrap();
    s.revert_clear().wait().unwrap();
    let credentials = s.credentials().wait().unwrap();
    assert_eq!(credentials[1].kind, CredentialKind::OAuth);
    assert!(!format!("{credentials:?}").contains("secret"));
    let limits = s.rate_limits().wait().unwrap();
    assert_eq!(limits[0].status, LimitStatus::Warning);
    wait("the windows in the thread", || {
        s.with_thread(|t| !t.rate_limits().is_empty() && t.usage().subscription)
    });
    let (tx, rx) = std::sync::mpsc::channel();
    s.info().then(move |info| drop(tx.send(info.map(|i| i.urls.len()))));
    assert_eq!(rx.recv_timeout(Duration::from_secs(10)).unwrap(), Ok(1));
    run.conforms();
    assert!(matches!(
        s.request("GET", "/api/nowhere", None).wait(),
        Err(SessionError::Server(ServerError { status: 404, .. }))
    ));
}

#[test]
fn stopping_closes_the_input_and_the_server_ends() {
    let run = ready("basic", "stop");
    run.session.stop(Duration::from_secs(2));
    wait("the end", || run.session.is_ended());
    wait("the ended event", || {
        run.events.has(|e| matches!(e, Event::Ended { .. }))
    });
    assert!(!run.events.has(|e| matches!(e, Event::Failed(_))));
    assert_eq!(run.session.set_mode("plan"), Err(SessionError::Closed));
}

#[test]
fn a_version_read_before_the_start_is_not_read_again() {
    let (mut launch, log) = launch("basic", "known-version");
    launch.version = term_opencode::Version::parse("opencode v1.0.0");
    let run = start(launch, log);
    wait("the refusal", || {
        run.events
            .has(|e| matches!(e, Event::Failed(SessionError::Unsupported { .. })))
    });
}
