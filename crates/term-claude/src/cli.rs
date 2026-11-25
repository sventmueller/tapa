//! One-off runs of the CLI's own subcommands for the overlays of the agents' configuration (E146): the writers the
//! VS Code extension uses (`edit-permission-rules`, `edit-hook`, `edit-memory-settings`, `edit-skill-overrides`, one
//! JSON edit on stdin and one JSON line on stdout), and `mcp`, `plugin` and `auth`. Each runs through the supervisor
//! with the login shell's environment (E10) on the calling thread, which is a worker's.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use term_process::{Ending, Kind, Supervisor};

/// How long one run may take; a plugin's installation fetches its marketplace.
const LIMIT: Duration = Duration::from_secs(120);

/// What `claude plugin` answered.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginAnswer {
    /// Whether it did what it was asked.
    pub done: bool,
    /// Its JSON line, which says why when it did not.
    pub json: Value,
}

/// Why a subcommand gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// It did not start.
    Start(String),
    /// It ran past its time limit and was stopped.
    TimedOut,
    /// This CLI has no such subcommand, as an older version lacks the writers.
    Unknown,
    /// It refused or failed, in its own words.
    Failed(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Start(why) => write!(f, "could not start: {why}"),
            CliError::TimedOut => f.write_str("no answer in time"),
            CliError::Unknown => f.write_str("this version has no such subcommand"),
            CliError::Failed(said) => f.write_str(said),
        }
    }
}

impl term_base::explain::Explain for CliError {
    fn explain(&self) -> String {
        use splat_i18n::{t, tf};
        match self {
            CliError::Start(why) => tf("Claude Code ließ sich nicht starten: {}", &[why]),
            CliError::TimedOut => t("Claude Code hat nicht rechtzeitig geantwortet.").to_string(),
            CliError::Unknown => t("Diese Version von Claude Code kann das hier nicht.").to_string(),
            CliError::Failed(said) => said.clone(),
        }
    }
}

/// A change of the permission rules, as `claude edit-permission-rules` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionEdit {
    /// Adds `rules` with `behavior` (`allow`, `ask`, `deny`) to the settings file `destination`.
    Add {
        /// The rules, such as `Bash(git push *)`.
        rules: Vec<String>,
        /// `allow`, `ask` or `deny`.
        behavior: String,
        /// `localSettings`, `projectSettings` or `userSettings`.
        destination: String,
    },
    /// Removes `rule` with `behavior` from the settings file `source`.
    Remove {
        /// The rule.
        rule: String,
        /// `allow`, `ask` or `deny`.
        behavior: String,
        /// Where it lives.
        source: String,
    },
}

impl PermissionEdit {
    fn json(&self) -> Value {
        match self {
            PermissionEdit::Add {
                rules,
                behavior,
                destination,
            } => json!({ "op": "add", "rules": rules, "behavior": behavior, "destination": destination }),
            PermissionEdit::Remove { rule, behavior, source } => {
                json!({ "op": "remove", "rule": rule, "behavior": behavior, "source": source })
            }
        }
    }
}

/// Which hook an edit replaces or removes: its event, matcher and definition as the listing gave them.
#[derive(Debug, Clone, PartialEq)]
pub struct HookAt {
    /// The event.
    pub event: String,
    /// The matcher; empty for none.
    pub matcher: String,
    /// The definition.
    pub hook: Value,
}

/// A change of the hooks, as `claude edit-hook` takes it; `source` is the settings file it lives in.
#[derive(Debug, Clone, PartialEq)]
pub enum HookEdit {
    /// Adds `hook` on `event` with `matcher`.
    Add {
        /// The settings file.
        source: String,
        /// The event.
        event: String,
        /// The matcher; empty for none.
        matcher: String,
        /// The definition, such as `{"type": "command", "command": "…"}`.
        hook: Value,
    },
    /// Puts `hook` in the place of `target`.
    Replace {
        /// The settings file.
        source: String,
        /// The event.
        event: String,
        /// The matcher.
        matcher: String,
        /// The new definition.
        hook: Value,
        /// The hook it replaces.
        target: HookAt,
    },
    /// Removes `target`.
    Remove {
        /// The settings file.
        source: String,
        /// The hook.
        target: HookAt,
    },
}

impl HookEdit {
    fn json(&self) -> Value {
        let at = |target: &HookAt| json!({ "event": target.event, "matcher": target.matcher, "hook": target.hook });
        match self {
            HookEdit::Add {
                source,
                event,
                matcher,
                hook,
            } => json!({ "op": "add", "source": source, "event": event, "matcher": matcher, "hook": hook }),
            HookEdit::Replace {
                source,
                event,
                matcher,
                hook,
                target,
            } => json!({ "op": "replace", "source": source, "event": event, "matcher": matcher, "hook": hook,
                "target": at(target) }),
            HookEdit::Remove { source, target } => {
                json!({ "op": "remove", "source": source, "event": target.event, "target": at(target) })
            }
        }
    }
}

/// What `claude auth status` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthStatus {
    /// Whether anyone is signed in.
    pub logged_in: bool,
    /// `none`, `claude.ai`, `oauth_token`, `api_key`, `api_key_helper` or `third_party`.
    pub auth_method: String,
    /// The account's address, when the CLI tells it.
    pub email: Option<String>,
}

/// The CLI to run subcommands of, in a folder, with the login shell's environment.
#[derive(Debug, Clone)]
pub struct Cli {
    program: PathBuf,
    env: Vec<(String, String)>,
    cwd: PathBuf,
}

impl Cli {
    /// The CLI at `program`, run in `cwd` with `env`.
    pub fn new(program: PathBuf, env: Vec<(String, String)>, cwd: PathBuf) -> Cli {
        Cli { program, env, cwd }
    }

    /// Changes the permission rules; the rules the CLI stored.
    pub fn edit_permission_rules(&self, edit: &PermissionEdit) -> Result<Value, CliError> {
        self.edit("edit-permission-rules", &edit.json())
    }

    /// Changes the hooks.
    pub fn edit_hook(&self, edit: &HookEdit) -> Result<Value, CliError> {
        self.edit("edit-hook", &edit.json())
    }

    /// Switches automatic memory on or off.
    pub fn set_auto_memory(&self, enabled: bool) -> Result<Value, CliError> {
        self.edit("edit-memory-settings", &json!({ "autoMemoryEnabled": enabled }))
    }

    /// Sets how the skill `name` is offered: `on`, `name-only`, `user-invocable-only` or `off`; `handles` as the
    /// skills listing gave them.
    pub fn set_skill_state(&self, name: &str, state: &str, handles: &Value) -> Result<Value, CliError> {
        let mut edit = json!({ "name": name, "state": state });
        if handles.as_object().is_some_and(|handles| !handles.is_empty()) {
            edit["handles"] = handles.clone();
        }
        self.edit("edit-skill-overrides", &edit)
    }

    /// Adds the MCP server `name` with the definition `definition` in `scope` (`local`, `project`, `user`).
    pub fn mcp_add(&self, name: &str, definition: &Value, scope: &str) -> Result<String, CliError> {
        let definition = definition.to_string();
        self.run(
            &["mcp", "add-json", "-s", scope, name, &definition],
            None,
            "claude mcp add-json",
        )
    }

    /// Removes the MCP server `name` from `scope`.
    pub fn mcp_remove(&self, name: &str, scope: &str) -> Result<String, CliError> {
        self.run(&["mcp", "remove", "-s", scope, name], None, "claude mcp remove")
    }

    /// Who is signed in.
    pub fn auth_status(&self) -> Result<AuthStatus, CliError> {
        let (ending, out, err) = self.run_raw(&["auth", "status"], None, "claude auth status")?;
        // Not signed in ends with 1 and still prints the status on stdout, whatever it warns on stderr.
        if let Some(status) = last_json(&out) {
            return serde_json::from_value(status).map_err(|e| CliError::Failed(e.to_string()));
        }
        let out = outcome(ending, out, err)?;
        Err(CliError::Failed(out.trim().to_string()))
    }

    /// Signs out of Claude Code.
    pub fn auth_logout(&self) -> Result<String, CliError> {
        self.run(&["auth", "logout"], None, "claude auth logout")
    }

    /// Runs `claude plugin` with `args` and `--json`; the JSON it prints last, also when it refused, as it then says
    /// why there, such as a marketplace's command waiting for the user's consent.
    pub fn plugin(&self, args: &[&str]) -> Result<PluginAnswer, CliError> {
        let mut all = vec!["plugin"];
        all.extend_from_slice(args);
        all.push("--json");
        let (ending, out, err) = self.run_raw(&all, None, "claude plugin")?;
        let done = matches!(ending, Some(Ending::Code(0)));
        if let Some(json) = last_json(&out) {
            return Ok(PluginAnswer { done, json });
        }
        outcome(ending, out, err).and_then(|out| Err(CliError::Failed(out.trim().to_string())))
    }

    /// Runs the writer `subcommand` with `edit` on stdin; what it answered.
    fn edit(&self, subcommand: &str, edit: &Value) -> Result<Value, CliError> {
        let out = self.run(&[subcommand, "--json"], Some(&edit.to_string()), subcommand)?;
        last_json(&out).ok_or_else(|| CliError::Failed(out.trim().to_string()))
    }

    /// Runs the CLI with `args`, `stdin` written to it, named `label` in the activity view, to its end on this
    /// thread.
    fn run(&self, args: &[&str], stdin: Option<&str>, label: &str) -> Result<String, CliError> {
        let (ending, out, err) = self.run_raw(args, stdin, label)?;
        outcome(ending, out, err)
    }

    /// Runs the CLI as `run` does; how it ended, its stdout and its stderr.
    fn run_raw(
        &self,
        args: &[&str],
        stdin: Option<&str>,
        label: &str,
    ) -> Result<(Option<Ending>, String, String), CliError> {
        let mut command = Command::new(&self.program);
        command
            .args(args)
            .env_clear()
            .envs(subcommand_environment(&self.env))
            .current_dir(&self.cwd)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // The label leaves the arguments out, which may carry an MCP server's tokens (R42).
        let supervisor = Supervisor::global();
        let mut spawned = supervisor
            .spawn_awaited(command, Kind::Agent, label)
            .map_err(|e| CliError::Start(e.to_string()))?;
        let (out, err, ending) = std::thread::scope(|scope| {
            let read = |pipe: Option<Box<dyn Read + Send>>| {
                scope.spawn(move || {
                    let mut text = String::new();
                    if let Some(mut pipe) = pipe {
                        let _ = pipe.read_to_string(&mut text);
                    }
                    text
                })
            };
            let out = read(spawned.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
            let err = read(spawned.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
            if let (Some(mut pipe), Some(text)) = (spawned.stdin.take(), stdin) {
                let _ = pipe.write_all(text.as_bytes());
            }
            let ending = supervisor.wait(spawned.id, LIMIT);
            if ending.is_none() {
                supervisor.stop(spawned.id, Duration::from_secs(1));
            }
            (out.join().unwrap_or_default(), err.join().unwrap_or_default(), ending)
        });
        Ok((ending, out, err))
    }
}

/// What a run's end and output mean.
fn outcome(ending: Option<Ending>, out: String, err: String) -> Result<String, CliError> {
    match ending {
        Some(Ending::Code(0)) => Ok(out),
        None => Err(CliError::TimedOut),
        Some(_) if is_unknown(&err) => Err(CliError::Unknown),
        Some(_) => {
            let said = if err.trim().is_empty() { out } else { err };
            Err(CliError::Failed(said.trim().to_string()))
        }
    }
}

/// Whether the CLI did not know the subcommand: it then reads the word as a prompt and refuses the `--json` after it.
fn is_unknown(err: &str) -> bool {
    err.contains("unknown option '--json'") || err.contains("unknown command")
}

/// The JSON on the last line of `out` that holds any.
fn last_json(out: &str) -> Option<Value> {
    out.lines()
        .rev()
        .find_map(|line| serde_json::from_str(line.trim()).ok())
}

/// `env` without what marks a process as running inside a Claude Code session, where the writers refuse to work
/// ("cannot be changed from an editor started inside a Claude Code session"); the configuration folder stays.
fn subcommand_environment(env: &[(String, String)]) -> Vec<(String, String)> {
    env.iter()
        .filter(|(key, _)| key != "CLAUDECODE" && key != "TRACEPARENT" && !key.starts_with("CLAUDE_CODE_"))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `claude` that writes its arguments and stdin to files and prints `answer`, exiting with `code`.
    fn stand_in(name: &str, answer: &str, code: i32) -> (PathBuf, Cli) {
        let dir = std::env::temp_dir().join(format!("term-claude-cli-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{dir}/args\"\ncat > \"{dir}/stdin\"\nprintf '%s\\n' '{answer}'\n\
             env > \"{dir}/env\"\nexit {code}\n",
            dir = dir.display()
        );
        let program = dir.join("claude");
        std::fs::write(&program, script).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = vec![
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            ("CLAUDE_CODE_CHILD_SESSION".to_string(), "1".to_string()),
            ("CLAUDE_CONFIG_DIR".to_string(), "/tmp/cfg".to_string()),
        ];
        let cli = Cli::new(program, env, dir.clone());
        (dir, cli)
    }

    #[test]
    fn a_writer_gets_its_edit_on_stdin_and_runs_outside_the_session() {
        let (dir, cli) = stand_in("edit", r#"{"ok":true,"stored":["Bash(ls *)"]}"#, 0);
        let edit = PermissionEdit::Add {
            rules: vec!["Bash(ls *)".into()],
            behavior: "allow".into(),
            destination: "localSettings".into(),
        };
        let answer = cli.edit_permission_rules(&edit).unwrap();
        assert_eq!(answer["stored"][0], "Bash(ls *)");
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(args, "edit-permission-rules\n--json\n");
        let stdin: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("stdin")).unwrap()).unwrap();
        assert_eq!(stdin, edit.json());
        let env = std::fs::read_to_string(dir.join("env")).unwrap();
        assert!(
            !env.contains("CLAUDECODE=") && !env.contains("CLAUDE_CODE_CHILD_SESSION="),
            "{env}"
        );
        assert!(env.contains("CLAUDE_CONFIG_DIR=/tmp/cfg"), "{env}");
    }

    #[test]
    fn hook_edits_name_the_hook_they_change() {
        let target = HookAt {
            event: "PreToolUse".into(),
            matcher: "Bash".into(),
            hook: json!({ "type": "command", "command": "echo check" }),
        };
        let remove = HookEdit::Remove {
            source: "userSettings".into(),
            target,
        };
        assert_eq!(
            remove.json(),
            json!({ "op": "remove", "source": "userSettings", "event": "PreToolUse",
                "target": { "event": "PreToolUse", "matcher": "Bash", "hook": { "type": "command", "command": "echo check" } } })
        );
    }

    #[test]
    fn a_refusal_an_unknown_writer_and_a_status_while_signed_out_read_apart() {
        let (_, refusing) = stand_in("refuse", "op must be one of: add, remove, compose", 1);
        assert_eq!(
            refusing.set_auto_memory(true),
            Err(CliError::Failed("op must be one of: add, remove, compose".into()))
        );
        assert_eq!(
            outcome(
                Some(Ending::Code(1)),
                String::new(),
                "error: unknown option '--json'\n".into()
            ),
            Err(CliError::Unknown)
        );
        let (_, signed_out) = stand_in("auth", r#"{"loggedIn": false, "authMethod": "none"}"#, 1);
        let status = signed_out.auth_status().unwrap();
        assert!(!status.logged_in && status.auth_method == "none");
    }

    #[test]
    fn the_last_json_line_is_the_answer() {
        assert_eq!(last_json("progress\n{\"a\":1}\n\n"), Some(json!({ "a": 1 })));
        assert_eq!(last_json("nothing here"), None);
    }

    #[test]
    fn a_plugin_refusal_still_gives_its_json() {
        let (dir, cli) = stand_in(
            "plugin",
            r#"{"ok":false,"shownCommand":{"command":"make","sha256":"ab"}}"#,
            1,
        );
        let answer = cli.plugin(&["install", "demo@market", "-s", "user"]).unwrap();
        assert!(!answer.done);
        assert_eq!(answer.json["shownCommand"]["sha256"], "ab");
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        assert_eq!(args, "plugin\ninstall\ndemo@market\n-s\nuser\n--json\n");
        let (_, silent) = stand_in("plugin-silent", "no such plugin", 1);
        assert_eq!(
            silent.plugin(&["enable", "x"]),
            Err(CliError::Failed("no such plugin".into()))
        );
    }

    #[test]
    fn a_signed_out_status_reads_from_stdout_whatever_the_cli_warns() {
        let (dir, _) = stand_in("auth-warn", "", 0);
        let script = "#!/bin/sh\nprintf '%s\\n' '{\"loggedIn\":false,\"authMethod\":\"none\"}'\n\
             echo 'a newer version is available' >&2\nexit 1\n";
        std::fs::write(dir.join("claude"), script).unwrap();
        let cli = Cli::new(dir.join("claude"), Vec::new(), dir.clone());
        let status = cli.auth_status().unwrap();
        assert!(!status.logged_in);
        assert_eq!(status.auth_method, "none");
    }
}
