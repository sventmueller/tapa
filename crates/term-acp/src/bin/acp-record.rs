//! Records what a client and an ACP agent say to each other, for replay tests: `acp-record <file> <program> [args…]`
//! runs the agent with this process in between and appends every message, in the order it passed, to `<file>` as
//! one JSON line `{"to": "agent" | "client", "message": …}`.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(file), Some(program)) = (args.next(), args.next()) else {
        eprintln!("usage: acp-record <file> <program> [args…]");
        return ExitCode::from(2);
    };
    match relay(&file, &program, args.collect()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("acp-record: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs `program` and passes its lines on, recording each; the agent's exit status as a byte.
fn relay(file: &OsString, program: &OsString, args: Vec<OsString>) -> std::io::Result<u8> {
    let log = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(file)?));
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let (Some(mut to_agent), Some(from_agent)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(std::io::Error::other("the agent's pipes are missing"));
    };
    let up = Arc::clone(&log);
    // Not joined: the client may keep its end open after the agent ended.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            record(&up, "agent", &line);
            if writeln!(to_agent, "{line}").and_then(|()| to_agent.flush()).is_err() {
                break;
            }
        }
    });
    let mut out = std::io::stdout().lock();
    for line in BufReader::new(from_agent).lines() {
        let Ok(line) = line else { break };
        record(&log, "client", &line);
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    let status = child.wait()?;
    Ok(status.code().map_or(1, |code| u8::try_from(code).unwrap_or(1)))
}

fn record(log: &Mutex<File>, to: &str, line: &str) {
    let message = serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_string()));
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "{}", json!({"to": to, "message": message}));
    }
}
