//! The stand-in server answers as the schema of OpenCode 2.2.6 describes: every operation the crate uses, the error
//! of a request without the password, and every event of the turns it plays.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use support::schema::Spec;
use term_opencode::test_server::{FORM, PERMISSION, SESSION};
use term_process::{Kind, Supervisor};

const PASSWORD: &str = "spec-password";

struct Server {
    address: SocketAddr,
    _stdin: std::process::ChildStdin,
    _supervisor: Supervisor,
}

fn serve() -> Server {
    let dir = std::env::temp_dir().join(format!("term-opencode-spec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-test-server"));
    command
        .args(["serve", "--stdio", "--port", "0"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("OPENCODE_SERVER_PASSWORD", PASSWORD)
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let supervisor = Supervisor::new();
    let spawned = supervisor.spawn(command, Kind::Agent, "opencode-test-server").unwrap();
    let mut line = String::new();
    BufReader::new(spawned.stdout.unwrap()).read_line(&mut line).unwrap();
    let url: Value = serde_json::from_str(&line).unwrap();
    let address = url["url"]
        .as_str()
        .unwrap()
        .trim_start_matches("http://")
        .parse()
        .unwrap();
    Server {
        address,
        _stdin: spawned.stdin.unwrap(),
        _supervisor: supervisor,
    }
}

fn authorization() -> String {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("opencode:{PASSWORD}"));
    format!("Basic {token}")
}

/// The body of a chunked answer.
fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let end = raw.windows(2).position(|w| w == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&raw[..end]).unwrap().trim(), 16).unwrap();
        if size == 0 {
            return body;
        }
        body.extend_from_slice(&raw[end + 2..end + 2 + size]);
        raw = &raw[end + 4 + size..];
    }
}

/// Sends a request and reads the whole answer.
fn send(server: &Server, method: &str, target: &str, body: Option<&Value>, authorized: bool) -> (u16, Value) {
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let text = body.map(Value::to_string).unwrap_or_default();
    let auth = if authorized {
        format!("Authorization: {}\r\n", authorization())
    } else {
        String::new()
    };
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: {}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        server.address,
        text.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head[9..12].parse().unwrap();
    let rest = &raw[split + 4..];
    let body = if head.contains("transfer-encoding: chunked") {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    (status, value)
}

/// The events of the stream, as they arrive.
fn subscribe(server: &Server) -> Receiver<Value> {
    let mut stream = TcpStream::connect(server.address).unwrap();
    write!(
        stream,
        "GET /api/event HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nAccept: text/event-stream\r\n\r\n",
        server.address,
        authorization()
    )
    .unwrap();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if let Some(data) = line.strip_prefix("data: ")
                && tx.send(serde_json::from_str(data).unwrap()).is_err()
            {
                return;
            }
        }
    });
    rx
}

/// The events until one of `kind`, which is included.
fn until(events: &Receiver<Value>, kind: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let event = events
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("no {kind}"));
        let done = event["type"] == kind;
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn conforms(spec: &Spec, server: &Server, method: &str, target: &str, body: Option<Value>) -> Value {
    if let Some(body) = &body {
        let path = target.split('?').next().unwrap();
        spec.check_request(method, path, &serde_json::Map::new(), body)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let (status, answer) = send(server, method, target, body.as_ref(), true);
    let path = target.split('?').next().unwrap();
    spec.check_response(method, path, status, &answer)
        .unwrap_or_else(|e| panic!("{e}"));
    answer
}

#[test]
fn every_answer_the_crate_reads_is_one_the_schema_describes() {
    let spec = Spec::load();
    let server = serve();
    assert!(
        spec.check_response("GET", "/api/info", 200, &json!({"version": 1}))
            .is_err()
    );
    assert!(
        spec.check_response("GET", "/api/agent", 200, &json!({"data": []}))
            .is_err(),
        "location is required"
    );
    let empty = serde_json::Map::new();
    assert!(
        spec.check_request("POST", "/api/session", &empty, &json!({"directory": "/w"}))
            .is_err()
    );
    assert!(
        spec.check_request("POST", "/api/session/abc/prompt", &empty, &json!({"text": "x"}))
            .is_err(),
        "ids start with ses"
    );
    let (status, refused) = send(&server, "GET", "/api/info", None, false);
    assert_eq!(status, 401);
    spec.check_response("GET", "/api/info", 401, &refused).unwrap();
    let location = "?location%5Bdirectory%5D=%2Fw";
    for path in [
        "/api/agent",
        "/api/model",
        "/api/model/default",
        "/api/provider",
        "/api/mcp",
        "/api/permission/request",
        "/api/experimental/usage/limits",
    ] {
        conforms(&spec, &server, "GET", &format!("{path}{location}"), None);
    }
    for path in [
        "/api/info",
        "/api/credential",
        "/api/session",
        "/api/session/ses_old",
        "/api/session/ses_old/message",
        "/api/session/ses_old/message?cursor=page2",
        "/api/session/ses_old/form",
        "/api/experimental/session/ses_old/export",
    ] {
        conforms(&spec, &server, "GET", path, None);
    }
    let created = conforms(
        &spec,
        &server,
        "POST",
        "/api/session",
        Some(json!({"location": {"directory": "/w"}})),
    );
    assert_eq!(created["data"]["id"], SESSION);
    let session = format!("/api/session/{SESSION}");
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/fork"),
        Some(json!({"before": "msg_a2"})),
    );
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/revert/stage"),
        Some(json!({"messageID": "msg_a1"})),
    );
    conforms(&spec, &server, "POST", &format!("{session}/revert/commit"), None);
    conforms(&spec, &server, "DELETE", &format!("{session}/revert"), None);
    conforms(
        &spec,
        &server,
        "PUT",
        &format!("/api/experimental/mcp/docs{location}"),
        Some(json!({"config": {"type": "remote", "url": "https://mcp.example.test"}})),
    );
    conforms(
        &spec,
        &server,
        "POST",
        &format!("/api/experimental/mcp/docs/connect{location}"),
        None,
    );
    conforms(
        &spec,
        &server,
        "DELETE",
        &format!("/api/experimental/mcp/docs{location}"),
        None,
    );
    let (status, missing) = send(&server, "GET", "/api/session/ses_missing", None, true);
    spec.check_response("GET", "/api/session/ses_missing", status, &missing)
        .unwrap();
}

#[test]
fn every_event_of_the_turns_is_one_the_schema_describes() {
    let spec = Spec::load();
    let server = serve();
    let events = subscribe(&server);
    let connected = events.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(connected["type"], "server.connected");
    let session = format!("/api/session/{SESSION}");
    conforms(&spec, &server, "POST", "/api/session", Some(json!({})));
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/prompt"),
        Some(json!({"text": "list the files"})),
    );
    let mut seen = until(&events, "permission.asked");
    assert_eq!(seen.last().unwrap()["data"]["id"], PERMISSION);
    let reply = format!("{session}/permission/{PERMISSION}/reply");
    conforms(&spec, &server, "POST", &reply, Some(json!({"decision": "once"})));
    seen.extend(until(&events, "session.idle"));
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/prompt"),
        Some(json!({"text": "ask me"})),
    );
    seen.extend(until(&events, "form.created"));
    let answer = format!("{session}/form/{FORM}/reply");
    conforms(
        &spec,
        &server,
        "POST",
        &answer,
        Some(json!({"answer": {"color": "blue"}})),
    );
    seen.extend(until(&events, "session.idle"));
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/prompt"),
        Some(json!({"text": "slow"})),
    );
    seen.extend(until(&events, "session.text.delta"));
    conforms(&spec, &server, "POST", &format!("{session}/interrupt"), None);
    seen.extend(until(&events, "session.idle"));
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/agent"),
        Some(json!({"agent": "plan"})),
    );
    seen.extend(until(&events, "session.agent.selected"));
    conforms(
        &spec,
        &server,
        "POST",
        &format!("{session}/model"),
        Some(json!({"model": {"providerID": "openai", "id": "gpt-5", "variant": "high"}})),
    );
    seen.extend(until(&events, "session.model.selected"));
    for event in &seen {
        spec.check_event(event).unwrap_or_else(|e| panic!("{e}"));
    }
    let kinds: std::collections::BTreeSet<&str> = seen.iter().filter_map(|e| e["type"].as_str()).collect();
    assert!(kinds.len() >= 25, "{kinds:?}");
}
