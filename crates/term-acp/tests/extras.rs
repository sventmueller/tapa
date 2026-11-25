//! What the overlays ask an ACP session besides the conversation (E146), against the test agent: the MCP servers go
//! over when the session opens, only in a trusted workspace, a question beside the conversation is answered in a fork that stays out of the thread
//! and runs no tools, and signing out reaches the agent.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use term_acp::mcp::{McpOrigin, McpServerSpec, McpTransport};
use term_acp::test_agent::{ASIDE_ANSWER, FORK};
use term_acp::{Launch, Session, Untrusted};
use term_base::testing::scratch;
use term_process::Supervisor;
use term_thread::{Thread, ThreadId};

fn ready(cwd: &Path, record: &Path, servers: Vec<McpServerSpec>) -> (Session, Supervisor) {
    ready_as(cwd, record, servers, true)
}

fn ready_as(cwd: &Path, record: &Path, servers: Vec<McpServerSpec>, trusted: bool) -> (Session, Supervisor) {
    let mut launch = Launch::new(
        "acp-test-agent",
        "Test Agent",
        env!("CARGO_BIN_EXE_acp-test-agent"),
        vec!["basic".into()],
        vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("ACP_TEST_RECORD".into(), record.to_string_lossy().into_owned()),
        ],
        cwd,
    );
    launch.mcp_servers = servers;
    launch.trusted = trusted;
    launch.untrusted = Untrusted::Env(&[("TEST_UNTRUSTED", "1")]);
    let supervisor = Supervisor::new();
    let thread = Thread::new(ThreadId::from("t"), launch.agent.clone(), launch.cwd.clone());
    let session = Session::start(&supervisor, launch, thread, |_| {}).unwrap();
    wait("a session", || session.is_ready());
    (session, supervisor)
}

fn wait(what: &str, check: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(Instant::now() < deadline, "waited in vain: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn recorded(record: &Path) -> String {
    std::fs::read_to_string(record).unwrap_or_default()
}

fn server(name: &str, transport: McpTransport) -> McpServerSpec {
    McpServerSpec {
        name: name.into(),
        transport,
        command: "sh".into(),
        args: vec!["-c".into(), "true".into()],
        url: "https://mcp.example".into(),
        env: vec![("TOKEN".into(), "t".into())],
        headers: Vec::new(),
        origin: McpOrigin::User,
        enabled: true,
    }
}

#[test]
fn the_mcp_servers_go_over_when_the_session_opens() {
    let dir = scratch("acp-extras-mcp");
    let record = dir.join("record");
    let servers = vec![
        server("prog", McpTransport::Stdio),
        server("web", McpTransport::Http),
        server("events", McpTransport::Sse),
    ];
    let (session, _supervisor) = ready(&dir, &record, servers);
    let sent = recorded(&record);
    let line = sent.lines().find(|l| l.starts_with("mcpServers: ")).unwrap();
    let json: serde_json::Value = serde_json::from_str(line.trim_start_matches("mcpServers: ")).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 2, "{json}");
    assert_eq!(json[0]["name"], "prog");
    assert_eq!(json[0]["command"], "/bin/sh");
    assert_eq!(json[1]["type"], "http");
    session.stop(Duration::from_millis(500));
}

#[test]
fn an_untrusted_workspace_hands_over_no_mcp_server() {
    let dir = scratch("acp-extras-untrusted");
    let record = dir.join("record");
    let (session, _supervisor) = ready_as(&dir, &record, vec![server("prog", McpTransport::Stdio)], false);
    let sent = recorded(&record);
    let line = sent.lines().find(|l| l.starts_with("mcpServers: ")).unwrap();
    assert_eq!(line, "mcpServers: []");
    session.stop(Duration::from_millis(500));
}

#[test]
fn a_question_beside_the_conversation_stays_out_of_the_thread_and_runs_no_tools() {
    let dir = scratch("acp-extras-aside");
    let record = dir.join("record");
    let (session, _supervisor) = ready(&dir, &record, Vec::new());
    assert!(session.capabilities().fork && session.capabilities().logout);
    let (tx, rx) = mpsc::channel();
    session
        .ask_aside("why red?", move |answer| tx.send(answer).unwrap())
        .unwrap();
    assert!(
        session.ask_aside("why blue?", |_| {}).is_err(),
        "a second question while the first forks must wait"
    );
    let answer = rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
    assert_eq!(answer, format!("{ASIDE_ANSWER}why red?"));
    let entries = format!("{:?}", session.snapshot());
    assert!(!entries.contains(ASIDE_ANSWER), "the fork's words reached the thread");
    wait("the fork closes", || {
        recorded(&record).contains(&format!("closed: \"{FORK}\""))
    });
    assert!(
        recorded(&record).contains("aside permission: \"cancelled\""),
        "{}",
        recorded(&record)
    );
    let (tx, rx) = mpsc::channel();
    session.logout(move |done| tx.send(done).unwrap()).unwrap();
    rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
    assert!(recorded(&record).contains("logout"));
    session.stop(Duration::from_millis(500));
}
