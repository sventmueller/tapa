//! How an agent is started: which program, with which arguments and environment, in which folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::install::Installs;
use crate::register::{Preset, Untrusted};
use crate::sandbox::Sandbox;

/// Where the program of an agent was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The agent's own command on the login shell's PATH, installed by the user.
    OnPath,
    /// A version the app installed.
    Installed {
        /// The version.
        version: String,
        /// Whether it was installed as a combination Dango verified.
        verified: bool,
    },
    /// Given directly, as in tests.
    Given,
}

/// Everything needed to start an agent.
#[derive(Debug, Clone, PartialEq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Launch {
    /// The register's id, which threads name as their agent.
    pub agent: String,
    /// The name shown.
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Its whole environment: the login shell's and the agent's own variables.
    pub env: Vec<(String, String)>,
    /// The project folder the session works in.
    pub cwd: PathBuf,
    /// The `_meta` of `session/new`.
    pub(crate) session_meta: Option<Value>,
    /// The agent's id of a session to load instead of starting a new one.
    pub resume: Option<String>,
    /// Where the program came from.
    pub origin: Origin,
    /// The MCP servers handed over when the session opens, only in a trusted workspace.
    pub mcp_servers: Vec<crate::mcp::McpServerSpec>,
    /// Whether the workspace is trusted (E16).
    pub trusted: bool,
    /// How the agent starts when the workspace is not trusted.
    pub untrusted: Untrusted,
}

impl Launch {
    /// `program` with `args` for `agent`, in `cwd` with the login shell's environment `env`.
    pub fn new(
        agent: impl Into<String>,
        name: impl Into<String>,
        program: impl Into<PathBuf>,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: impl Into<PathBuf>,
    ) -> Launch {
        Launch {
            agent: agent.into(),
            name: name.into(),
            program: program.into(),
            args,
            env,
            cwd: cwd.into(),
            session_meta: None,
            resume: None,
            origin: Origin::Given,
            mcp_servers: Vec::new(),
            trusted: false,
            untrusted: Untrusted::Refused,
        }
    }

    /// How `preset` starts in the workspace `cwd`: its own command on the login shell's PATH, else the version the
    /// app installed, never a program inside `cwd` (E16); nothing when it is neither.
    pub fn of_preset(
        preset: &Preset,
        installs: &Installs,
        env: &[(String, String)],
        cwd: &Path,
        trusted: bool,
    ) -> Option<Launch> {
        let Some(program) = preset.command(env, cwd) else {
            let mut launch = Launch::installed(preset.id, preset.name, installs, env, cwd, trusted)?;
            launch.untrusted = preset.untrusted;
            return Some(launch);
        };
        let args = preset.on_path.1.iter().map(|a| (*a).to_string()).collect();
        let mut launch = Launch::new(preset.id, preset.name, program, args, env.to_vec(), cwd);
        launch.origin = Origin::OnPath;
        launch.trusted = trusted;
        launch.untrusted = preset.untrusted;
        Some(launch)
    }

    /// How the app's installed version of `agent` starts in `cwd`; nothing when there is none. It starts only in a
    /// trusted workspace, as no switch of it to skip the project's configuration is known.
    pub fn installed(
        agent: &str,
        name: &str,
        installs: &Installs,
        env: &[(String, String)],
        cwd: &Path,
        trusted: bool,
    ) -> Option<Launch> {
        let (installed, program) = installs.current(agent)?;
        if term_process::tool::lies_in(&program, cwd) {
            return None;
        }
        let mut all = env.to_vec();
        for (key, value) in &installed.env {
            all.retain(|(k, _)| k != key);
            all.push((key.clone(), value.clone()));
        }
        let mut launch = Launch::new(agent, name, program, installed.args.clone(), all, cwd);
        launch.origin = Origin::Installed {
            version: installed.version,
            verified: installed.verified,
        };
        launch.trusted = trusted;
        Some(launch)
    }

    /// Whether the agent may start: in a trusted workspace, or in another when it can skip the project's
    /// configuration.
    pub fn may_start(&self) -> bool {
        self.trusted || self.untrusted != Untrusted::Refused
    }

    /// The MCP servers to hand over: none in a workspace that is not trusted.
    pub(crate) fn handed_servers(&self) -> &[crate::mcp::McpServerSpec] {
        if self.trusted { &self.mcp_servers } else { &[] }
    }

    /// The environment: the given one, with the variables that skip the project's configuration when the workspace
    /// is not trusted.
    pub(crate) fn environment(&self) -> Vec<(String, String)> {
        let mut env = self.env.clone();
        if let (false, Untrusted::Env(extra)) = (self.trusted, self.untrusted) {
            for (key, value) in extra {
                env.retain(|(k, _)| k != key);
                env.push(((*key).to_string(), (*value).to_string()));
            }
        }
        env
    }

    /// The same launch with `sandbox` switched on.
    pub fn sandboxed(mut self, sandbox: &Sandbox) -> Launch {
        self.args.extend(sandbox.args());
        for (key, value) in sandbox.env() {
            self.env.retain(|(k, _)| *k != key);
            self.env.push((key, value));
        }
        if let Some(meta) = sandbox.session_meta() {
            self.session_meta = Some(merge(self.session_meta.take(), meta));
        }
        self
    }

    /// The command that starts the agent with its pipes open.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .env_clear()
            .envs(self.environment())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// `extra` laid over `base`, object by object.
fn merge(base: Option<Value>, extra: Value) -> Value {
    match (base, extra) {
        (Some(Value::Object(mut base)), Value::Object(extra)) => {
            for (key, value) in extra {
                let merged = merge(base.remove(&key), value);
                base.insert(key, merged);
            }
            Value::Object(base)
        }
        (_, extra) => extra,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_sandbox_adds_its_switch_where_the_agent_reads_it() {
        let launch = Launch::new(
            "a",
            "A",
            "/bin/a",
            vec!["acp".into()],
            vec![("PATH".into(), "/bin".into())],
            "/w",
        );
        let launch = launch
            .sandboxed(&Sandbox::Args(&["--sandbox"]))
            .sandboxed(&Sandbox::Env(&[("MODE", "workspace-write")]))
            .sandboxed(&Sandbox::SessionMeta(
                r#"{"claudeCode":{"options":{"sandbox":{"enabled":true}}}}"#,
            ));
        assert_eq!(launch.args, ["acp", "--sandbox"]);
        assert!(launch.env.contains(&("MODE".into(), "workspace-write".into())));
        assert_eq!(
            launch.session_meta.unwrap()["claudeCode"]["options"]["sandbox"]["enabled"],
            true
        );
    }

    #[test]
    fn an_untrusted_workspace_hands_over_no_mcp_server_and_skips_the_projects_configuration() {
        let mut launch = Launch::new(
            "opencode",
            "OpenCode",
            "/bin/opencode",
            vec!["acp".into()],
            Vec::new(),
            "/w",
        );
        launch.mcp_servers.push(crate::mcp::McpServerSpec {
            name: "project".into(),
            transport: crate::mcp::McpTransport::Stdio,
            command: "./mcp".into(),
            args: Vec::new(),
            url: String::new(),
            env: Vec::new(),
            headers: Vec::new(),
            origin: crate::mcp::McpOrigin::Project,
            enabled: true,
        });
        assert!(
            !launch.may_start(),
            "an agent without a switch starts only when trusted"
        );
        launch.untrusted = Untrusted::Env(&[("OPENCODE_DISABLE_PROJECT_CONFIG", "true")]);
        assert!(launch.may_start());
        assert!(launch.handed_servers().is_empty());
        assert!(
            launch
                .environment()
                .contains(&("OPENCODE_DISABLE_PROJECT_CONFIG".into(), "true".into()))
        );
        launch.trusted = true;
        assert_eq!(launch.handed_servers().len(), 1);
        assert!(launch.environment().is_empty());
        for preset in crate::register::PRESETS {
            let refused = preset.untrusted == Untrusted::Refused;
            assert_eq!(refused, preset.id != "opencode", "{}", preset.id);
        }
    }

    #[test]
    fn meta_objects_merge_key_by_key() {
        let merged = merge(Some(json!({"a": {"x": 1}, "b": 2})), json!({"a": {"y": 3}}));
        assert_eq!(merged, json!({"a": {"x": 1, "y": 3}, "b": 2}));
    }
}
