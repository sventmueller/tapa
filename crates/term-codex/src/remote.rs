//! Codex's Remote Control: the phone reaches only Codex's shared app-server, the daemon that
//! `codex app-server daemon` manages, through OpenAI. A session that joins it through `codex app-server proxy`
//! (`Launch::shared`) runs on the Mac and in Codex on the phone at once. Remote Control belongs to that daemon, so it
//! is on or off for every Codex session of the Mac, not for one thread.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use term_process::Supervisor;
use term_process::tool::Helper;

use crate::protocol::text;
use crate::session::SessionError;

/// How long starting the daemon may take; a first start copies the Codex release it runs.
const START_TIMEOUT: Duration = Duration::from_secs(90);

/// How long pairing may take; the daemon waits up to 30 seconds for OpenAI.
const PAIR_TIMEOUT: Duration = Duration::from_secs(45);

/// Where the shared server stands with Remote Control.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RemoteControl {
    /// The session runs in a server of its own, which no phone reaches, or the shared one has Remote Control off.
    #[default]
    Off,
    /// The shared server connects to OpenAI.
    Connecting,
    /// The phone reaches the shared server, under the name it shows for the Mac.
    Connected(String),
    /// The shared server lost its connection to OpenAI.
    Failed,
}

impl RemoteControl {
    /// The state a `remoteControl/status/changed` notification or a `remoteControl/*` answer names.
    pub(crate) fn read(status: &Value) -> RemoteControl {
        match text(status, "status") {
            Some("connecting") => RemoteControl::Connecting,
            Some("connected") => RemoteControl::Connected(text(status, "serverName").unwrap_or_default().to_string()),
            Some("errored") => RemoteControl::Failed,
            _ => RemoteControl::Off,
        }
    }
}

/// A short-lived code that pairs a phone with the shared server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    /// What the user types into Codex on the phone.
    pub code: String,
    /// When the code stops working.
    pub expires: SystemTime,
}

/// The folder of Codex's own files: `CODEX_HOME` of `env`, else `.codex` in the home folder.
pub fn codex_home(env: &[(String, String)]) -> Option<PathBuf> {
    let var = |name: &str| env.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    match var("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(home) => Some(PathBuf::from(home)),
        None => var("HOME")
            .filter(|v| !v.is_empty())
            .map(|home| Path::new(home).join(".codex")),
    }
}

/// Whether the user turned Remote Control on: the daemon's saved choice, which `codex remote-control start` and the
/// Codex app write to `app-server-daemon/settings.json` and the daemon reads at each start.
pub fn wanted(env: &[(String, String)]) -> bool {
    codex_home(env)
        .and_then(|home| std::fs::read(home.join("app-server-daemon/settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|settings| settings.get("remoteControlEnabled").and_then(Value::as_bool))
        .unwrap_or(false)
}

/// Starts the shared server unless it runs, with its saved choice: `codex app-server daemon start`. This waits for
/// the process, so it runs off the main thread.
pub fn ensure_daemon(supervisor: &Supervisor, program: &Path, env: &[(String, String)]) -> Result<(), SessionError> {
    run(
        supervisor,
        program,
        env,
        &["app-server", "daemon", "start"],
        START_TIMEOUT,
    )
    .map(drop)
}

/// Starts the shared server with Remote Control on and keeps that choice, as `codex remote-control start` does; the
/// answer is where Remote Control stands once it connected or gave up waiting. This waits for the process.
pub fn start(supervisor: &Supervisor, program: &Path, env: &[(String, String)]) -> Result<RemoteControl, SessionError> {
    let out = run(
        supervisor,
        program,
        env,
        &["remote-control", "start", "--json"],
        START_TIMEOUT,
    )?;
    Ok(RemoteControl::read(&last_json(&out)?))
}

/// A manual pairing code for the phone, from the running shared server: `codex remote-control pair`. This waits for
/// the process.
pub fn pair(supervisor: &Supervisor, program: &Path, env: &[(String, String)]) -> Result<Pairing, SessionError> {
    let out = run(
        supervisor,
        program,
        env,
        &["remote-control", "pair", "--json"],
        PAIR_TIMEOUT,
    )?;
    read_pairing(&last_json(&out)?)
}

fn read_pairing(answer: &Value) -> Result<Pairing, SessionError> {
    let code = text(answer, "manualPairingCode")
        .or_else(|| text(answer, "pairingCode"))
        .ok_or_else(|| SessionError::Malformed("the pairing named no code".into()))?;
    let expires = answer
        .get("expiresAt")
        .and_then(Value::as_u64)
        .and_then(|at| UNIX_EPOCH.checked_add(Duration::from_secs(at)))
        .unwrap_or(UNIX_EPOCH);
    Ok(Pairing {
        code: code.to_string(),
        expires,
    })
}

/// Runs `program` with `args` through `supervisor`; its standard output when it succeeded, else what it said.
fn run(
    supervisor: &Supervisor,
    program: &Path,
    env: &[(String, String)],
    args: &[&str],
    timeout: Duration,
) -> Result<String, SessionError> {
    if !program.is_file() {
        return Err(SessionError::NotFound(program.to_path_buf()));
    }
    let helper = Helper {
        supervisor,
        env,
        timeout,
        cancel: None,
    };
    let output = helper
        .run(program, args, Path::new("/"), &mut |_| {})
        .map_err(|e| SessionError::Start(e.to_string()))?;
    if output.succeeded() {
        return Ok(output.stdout);
    }
    let said = [output.stderr.trim(), output.stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or_else(|| format!("{:?}", output.ending), str::to_string);
    Err(SessionError::Start(said))
}

/// The last line of `out` that is a JSON object, as the CLI prints its answer after any progress.
fn last_json(out: &str) -> Result<Value, SessionError> {
    out.lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line.trim()).ok().filter(Value::is_object))
        .ok_or_else(|| SessionError::Malformed("the CLI returned no JSON answer".into()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_saved_choice_is_read_from_codex_home_or_the_home_folder() {
        let dir = std::env::temp_dir().join(format!("term-codex-remote-{}", std::process::id()));
        let home = dir.join("codex");
        std::fs::create_dir_all(home.join("app-server-daemon")).unwrap();
        let env = vec![("CODEX_HOME".to_string(), home.to_string_lossy().into_owned())];
        assert!(!wanted(&env), "no settings, no Remote Control");
        std::fs::write(
            home.join("app-server-daemon/settings.json"),
            r#"{"remoteControlEnabled": true}"#,
        )
        .unwrap();
        assert!(wanted(&env));
        std::fs::write(
            home.join("app-server-daemon/settings.json"),
            r#"{"remoteControlEnabled": false}"#,
        )
        .unwrap();
        assert!(!wanted(&env));
        let env = vec![("HOME".to_string(), "/Users/me".to_string())];
        assert_eq!(codex_home(&env), Some(PathBuf::from("/Users/me/.codex")));
        assert_eq!(codex_home(&[]), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_answers_of_the_cli_and_the_server_read() {
        let started = "Starting app-server daemon with remote control enabled...\n\
            {\"mode\":\"daemon\",\"status\":\"connected\",\"serverName\":\"studio\",\"environmentId\":\"env_1\",\
            \"timedOut\":false}\n";
        assert_eq!(
            RemoteControl::read(&last_json(started).unwrap()),
            RemoteControl::Connected("studio".into())
        );
        assert_eq!(
            RemoteControl::read(&json!({"status": "errored", "serverName": "studio"})),
            RemoteControl::Failed
        );
        assert_eq!(RemoteControl::read(&json!({"status": "disabled"})), RemoteControl::Off);
        assert_eq!(
            RemoteControl::read(&json!({"status": "connecting"})),
            RemoteControl::Connecting
        );
        assert!(last_json("Pairing code: 1234").is_err());
        let pairing = read_pairing(&json!({"pairingCode": "long", "manualPairingCode": "ABCD-1234",
                                           "environmentId": "env_1", "expiresAt": 2_000_000_000}))
        .unwrap();
        assert_eq!(pairing.code, "ABCD-1234");
        assert_eq!(pairing.expires, UNIX_EPOCH + Duration::from_secs(2_000_000_000));
        assert!(read_pairing(&json!({"environmentId": "env_1"})).is_err());
    }
}
