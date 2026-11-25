//! The stand-ins the self-test's agent tabs talk to instead of the real CLIs, so that no agent installed on the Mac
//! ever starts and nothing leaves it: a shell script as `claude` that keeps its arguments and every line the app
//! writes and plays a turn with an edit to approve, and this program itself as Codex's app-server, OpenCode's server
//! and an ACP agent, which it becomes with `TAPA_STAND_IN` set.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The variable that makes this program a stand-in: `codex`, `opencode` or `acp:<scenario>`.
const VARIABLE: &str = "TAPA_STAND_IN";

/// The session id of the stand-in's frames.
const SESSION: &str = "51e1f7e5-4000-4000-8000-0000000000aa";

const SCRIPT: &str = r#"#!/bin/sh
dir='@DIR@'
printf '%s\n' "$@" > "$dir/args.txt"
turns=0
answers=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$dir/written.ndjson"
  case "$line" in
    *'"type":"control_request"'*)
      id=$(printf '%s\n' "$line" | /usr/bin/sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
      kind=$(printf '%s\n' "$line" | /usr/bin/sed -n 's/.*"subtype":"\([^"]*\)".*/\1/p')
      payload='{}'
      [ -f "$dir/answer-$kind.json" ] && payload=$(/bin/cat "$dir/answer-$kind.json")
      printf '{"type":"control_response","response":{"subtype":"success","request_id":"%s","response":%s}}\n' "$id" "$payload"
      ;;
    *'"type":"control_response"'*)
      answers=$((answers + 1))
      [ -f "$dir/after-$answers.ndjson" ] && /bin/cat "$dir/after-$answers.ndjson"
      ;;
    *'"type":"user"'*)
      turns=$((turns + 1))
      [ -f "$dir/turn-$turns.ndjson" ] && /bin/cat "$dir/turn-$turns.ndjson"
      if [ -f "$dir/turn-$turns.later.ndjson" ]; then /bin/sleep 1.5; /bin/cat "$dir/turn-$turns.later.ndjson"; fi
      ;;
  esac
done
"#;

/// Becomes the stand-in `TAPA_STAND_IN` names; the exit code, or none when the variable is not set.
pub(crate) fn run() -> Option<i32> {
    let which = std::env::var(VARIABLE).ok()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match which.as_str() {
        "codex" => term_codex::test_server::main(&args),
        "opencode" => {
            let scenario =
                std::env::var(term_opencode::test_server::SCENARIO_VARIABLE).unwrap_or_else(|_| "basic".into());
            if args.iter().any(|arg| arg == "--version") {
                println!("{}", term_opencode::test_server::version(&scenario));
            } else {
                let log = std::env::var_os(term_opencode::test_server::LOG_VARIABLE).map(PathBuf::from);
                term_opencode::test_server::run(std::io::stdin(), std::io::stdout().lock(), &scenario, log);
            }
            0
        }
        other => match other.strip_prefix("acp:") {
            Some(scenario) => {
                term_acp::test_agent::run(std::io::stdin().lock(), std::io::stdout().lock(), scenario);
                0
            }
            None => 2,
        },
    };
    Some(code)
}

/// The folder of the stand-ins: the script, its frames and what it saw.
pub(crate) fn dir() -> PathBuf {
    std::env::var_os("TAPA_SELFTEST_DIR")
        .map_or_else(|| PathBuf::from("target/selftest"), PathBuf::from)
        .join("agent-stand-in")
}

/// Writes the `claude` stand-in for a workspace in `cwd`; its program.
pub(crate) fn claude(cwd: &Path) -> std::io::Result<PathBuf> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let program = dir.join("claude");
    std::fs::write(&program, SCRIPT.replace("@DIR@", &dir.to_string_lossy()))?;
    std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    std::fs::write(dir.join("answer-initialize.json"), initialize().to_string())?;
    let (now, later) = first_turn(cwd);
    frames(&dir.join("turn-1.ndjson"), &now)?;
    frames(&dir.join("turn-1.later.ndjson"), &later)?;
    frames(&dir.join("after-1.ndjson"), &after_answer(cwd))?;
    Ok(program)
}

/// The arguments the `claude` stand-in was last started with.
pub(crate) fn claude_args() -> Vec<String> {
    std::fs::read_to_string(dir().join("args.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The environment the program's own stand-ins start with: `which` and a PATH without anything installed.
pub(crate) fn env(which: &str) -> Vec<(String, String)> {
    vec![
        (VARIABLE.into(), which.into()),
        ("PATH".into(), "/usr/bin:/bin".into()),
        (
            "CODEX_TEST_ARGS".into(),
            dir().join("codex-args.ndjson").to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".into(),
            dir().join("codex-home").to_string_lossy().into_owned(),
        ),
    ]
}

fn frames(path: &Path, frames: &[Value]) -> std::io::Result<()> {
    let text: String = frames.iter().map(|frame| format!("{frame}\n")).collect();
    std::fs::write(path, text)
}

fn initialize() -> Value {
    json!({
        "commands": [],
        "models": [{"value": "default", "displayName": "Opus", "description": "Most capable"}],
        "account": {"email": "selftest@example.org", "subscriptionType": "max", "tokenSource": "claude.ai"},
        "output_style": "default",
        "available_output_styles": ["default"]
    })
}

fn uuid(n: u32) -> String {
    format!("51e1f7e5-5000-4000-8000-{n:012}")
}

fn message(n: u32, content: &Value) -> Value {
    json!({"type": "assistant", "message": {"model": "claude-opus-5-5", "id": format!("msg_tapa_{n}"),
           "type": "message", "role": "assistant", "content": content,
           "stop_reason": null, "usage": {"input_tokens": 1, "output_tokens": 1}},
           "parent_tool_use_id": null, "session_id": SESSION, "uuid": uuid(n)})
}

/// The turn's start, an edit of `hello.txt`, and a moment later, so that the turn shows as running first, its ask
/// for approval.
fn first_turn(cwd: &Path) -> (Vec<Value>, Vec<Value>) {
    let input = json!({"file_path": cwd.join("hello.txt"), "old_string": "hello", "new_string": "hello there"});
    let now = vec![
        json!({"type": "system", "subtype": "init", "cwd": cwd, "session_id": SESSION, "tools": ["Edit"],
               "mcp_servers": [], "model": "claude-opus-5-5", "permissionMode": "default", "slash_commands": [],
               "apiKeySource": "none", "claude_code_version": "2.1.319", "output_style": "default",
               "uuid": uuid(1)}),
        message(
            2,
            &json!([{"type": "tool_use", "id": "toolu_edit", "name": "Edit", "input": input}]),
        ),
    ];
    let later = vec![
        json!({"type": "control_request", "request_id": "p1", "request": {"subtype": "can_use_tool",
               "tool_name": "Edit", "display_name": "Edit", "input": input, "tool_use_id": "toolu_edit",
               "permission_suggestions": [{"type": "addRules", "rules": [{"toolName": "Edit",
                   "ruleContent": "hello.txt"}], "behavior": "allow", "destination": "localSettings"}]}}),
    ];
    (now, later)
}

/// What follows the answer: the edit's result and a short reply that ends the turn.
fn after_answer(cwd: &Path) -> Vec<Value> {
    vec![
        json!({"type": "user", "message": {"role": "user", "content": [{"tool_use_id": "toolu_edit",
               "type": "tool_result", "content": format!("Edited {}", cwd.join("hello.txt").display())}]},
               "parent_tool_use_id": null, "session_id": SESSION, "uuid": uuid(3)}),
        message(
            4,
            &json!([{"type": "text", "text": "Erledigt: hello.txt sagt jetzt hello there."}]),
        ),
        json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 900,
               "duration_api_ms": 800, "num_turns": 1, "result": "ok", "stop_reason": "end_turn",
               "session_id": SESSION, "total_cost_usd": 0.0123, "usage": {"input_tokens": 100, "output_tokens": 20},
               "uuid": uuid(5)}),
    ]
}
