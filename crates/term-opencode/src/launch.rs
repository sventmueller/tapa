//! How OpenCode is started, the password of one start, and which agent and model its session opens with.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use base64::Engine;
use serde_json::{Value, json};

use crate::capabilities::ModelRef;

/// The variable the server takes its password from.
pub const PASSWORD_VARIABLE: &str = "OPENCODE_SERVER_PASSWORD";

/// The variable OpenCode prefers over `PASSWORD_VARIABLE`, which the login shell must not hand it.
const PREFERRED_VARIABLE: &str = "OPENCODE_PASSWORD";

/// What an untrusted workspace sets (E16): OpenCode 2.4 then reads neither the project's `opencode.json`,
/// `.opencode/` and `AGENTS.md` nor its `.claude/` folder, so no project agent, plugin or MCP server loads.
pub const UNTRUSTED_VARIABLES: [(&str, &str); 2] = [
    ("OPENCODE_DISABLE_PROJECT_CONFIG", "true"),
    ("OPENCODE_DISABLE_CLAUDE_CODE", "true"),
];

/// The random password of one server; it never appears in a log, a label or `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// 32 random bytes from the system's generator, as unpadded URL-safe base64.
    pub(crate) fn generate() -> std::io::Result<Secret> {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(Secret(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

/// Everything needed to start OpenCode and open its session.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The name shown in the activity view.
    pub name: String,
    /// The `opencode` executable.
    pub program: PathBuf,
    /// Its whole environment: the login shell's (E10, E23); a password in it is replaced by the session's own.
    pub env: Vec<(String, String)>,
    /// The project folder the server and its session work in.
    pub cwd: PathBuf,
    /// The agent to start with, such as `build` or `plan`; OpenCode's default when `None`.
    pub agent: Option<String>,
    /// The model to start with as `provider/model`; the agent's or OpenCode's default when `None`.
    pub model: Option<String>,
    /// OpenCode's id of a session to resume instead of creating one, such as `ses_…`.
    pub resume: Option<String>,
    /// The release `version::probe` already read for this program; the handshake reads it itself when `None`.
    pub version: Option<crate::version::Version>,
    /// Whether the workspace is trusted (E16); an untrusted one starts the server with `UNTRUSTED_VARIABLES`.
    pub trusted: bool,
}

impl Launch {
    /// `program` in `cwd` with the login shell's environment `env`, with OpenCode's own defaults.
    pub fn new(program: impl Into<PathBuf>, env: Vec<(String, String)>, cwd: impl Into<PathBuf>) -> Launch {
        Launch {
            name: "OpenCode".into(),
            program: program.into(),
            env,
            cwd: cwd.into(),
            agent: None,
            model: None,
            resume: None,
            version: None,
            trusted: false,
        }
    }

    /// The arguments: the server on a free port of 127.0.0.1, ending when its standard input closes.
    pub fn args(&self) -> Vec<String> {
        ["serve", "--stdio", "--port", "0"].map(String::from).to_vec()
    }

    /// The command that starts the server with its pipes open and `password` as the only one it accepts.
    pub(crate) fn command(&self, password: &Secret) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(self.args())
            .env_clear()
            .envs(
                self.env
                    .iter()
                    .filter(|(k, _)| k != PASSWORD_VARIABLE && k != PREFERRED_VARIABLE)
                    .map(|(k, v)| (k.as_str(), v.as_str())),
            )
            .envs(UNTRUSTED_VARIABLES.iter().copied().filter(|_| !self.trusted))
            .env(PASSWORD_VARIABLE, password.expose())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// The body of `POST /api/session` for a new session in the project folder.
    pub(crate) fn create_body(&self) -> Value {
        let mut body = json!({"location": {"directory": self.cwd.to_string_lossy()}});
        if let Some(agent) = &self.agent {
            body["agent"] = json!(agent);
        }
        if let Some(model) = self.model.as_deref().and_then(ModelRef::parse) {
            body["model"] = model.json();
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_gets_its_own_password_and_none_from_the_shell() {
        let env = vec![
            ("PATH".into(), "/bin".into()),
            (PREFERRED_VARIABLE.into(), "from-shell".into()),
            (PASSWORD_VARIABLE.into(), "also-from-shell".into()),
        ];
        let launch = Launch::new("/bin/opencode", env, "/work");
        let secret = Secret::generate().unwrap();
        assert_eq!(secret.expose().len(), 43);
        assert_ne!(secret, Secret::generate().unwrap());
        assert_eq!(format!("{secret:?}"), "Secret(..)");
        let command = launch.command(&secret);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&(PASSWORD_VARIABLE.into(), Some(secret.expose().into()))));
        assert!(!envs.iter().any(|(k, _)| k == PREFERRED_VARIABLE));
        assert!(!envs.iter().any(|(_, v)| v.as_deref() == Some("also-from-shell")));
        assert_eq!(launch.args(), ["serve", "--stdio", "--port", "0"]);
    }

    #[test]
    fn an_untrusted_workspace_starts_the_server_without_the_projects_configuration() {
        let mut launch = Launch::new("/bin/opencode", Vec::new(), "/work");
        let secret = Secret::generate().unwrap();
        let envs = |command: &Command| -> Vec<(String, String)> {
            command
                .get_envs()
                .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
                .collect()
        };
        let untrusted = envs(&launch.command(&secret));
        for (name, value) in UNTRUSTED_VARIABLES {
            assert!(
                untrusted.contains(&(name.into(), value.into())),
                "{name} is missing from {untrusted:?}"
            );
        }
        launch.trusted = true;
        let trusted = envs(&launch.command(&secret));
        assert!(
            !trusted.iter().any(|(k, _)| k.starts_with("OPENCODE_DISABLE")),
            "{trusted:?}"
        );
    }

    #[test]
    fn a_new_session_names_its_folder_agent_and_model() {
        let mut launch = Launch::new("/bin/opencode", Vec::new(), "/work");
        assert_eq!(launch.create_body(), json!({"location": {"directory": "/work"}}));
        launch.agent = Some("plan".into());
        launch.model = Some("openrouter/anthropic/claude-sonnet-4".into());
        assert_eq!(
            launch.create_body(),
            json!({"location": {"directory": "/work"}, "agent": "plan",
                   "model": {"providerID": "openrouter", "id": "anthropic/claude-sonnet-4"}})
        );
    }
}
