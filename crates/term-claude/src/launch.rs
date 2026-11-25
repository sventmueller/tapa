//! How the `claude` process is started: its arguments and environment.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use term_thread::Rewind;

/// How much the model thinks before it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Thinking {
    /// As the CLI and the user's settings decide.
    #[default]
    Default,
    /// As much as the model finds useful.
    Adaptive,
    /// Up to this many tokens.
    Budget(u32),
    /// Not at all, where the model allows it.
    Disabled,
}

/// Everything that decides how a Claude session starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The `claude` executable.
    pub(crate) executable: PathBuf,
    /// The login shell's environment, which the process gets instead of the app's.
    pub env: Vec<(String, String)>,
    /// The folder the agent works in.
    pub cwd: PathBuf,
    /// The model, by alias or full name.
    pub model: Option<String>,
    /// The permission mode; new threads start in `default`, where the agent asks.
    pub permission_mode: Option<String>,
    /// The reasoning effort: `low`, `medium`, `high`, `xhigh` or `max`.
    pub effort: Option<String>,
    /// How much the model thinks.
    pub thinking: Thinking,
    /// How thinking is shown: `summarized` or `omitted`.
    pub(crate) thinking_display: Option<String>,
    /// The session to resume, by id.
    pub resume: Option<String>,
    /// The id a new session takes, a UUID, so it can be resumed before its first turn named it; only without
    /// `resume`.
    pub session_id: Option<String>,
    /// The message of the resumed session up to which it is resumed, by uuid; only with `resume`.
    pub(crate) resume_at: Option<String>,
    /// The prompt whose turn resuming at `resume_at` drops; the CLI refuses to resume when more would be dropped. Only
    /// with `resume` and `resume_at`.
    pub(crate) resume_drops_turn: Option<String>,
    /// Whether the resumed session continues under a new session id, as a branch; only with `resume`.
    pub(crate) fork_session: bool,
    /// Tools allowed without asking, as permission rules such as `Bash(git status:*)`.
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) disallowed_tools: Vec<String>,
    /// More folders the tools may use.
    pub(crate) add_dirs: Vec<PathBuf>,
    /// Whether the session may switch to `bypassPermissions` while it runs, which the CLI refuses unless told so at
    /// launch; it does not start in that mode for it.
    pub allow_bypass: bool,
    /// Whether shell commands run in the CLI's sandbox.
    pub sandbox: bool,
    /// Whether the CLI saves the transcript, which resuming and the session list need.
    pub persist: bool,
    /// Whether the CLI keeps file backups so that `rewind_files` works.
    pub file_checkpoints: bool,
    /// Whether subagents report a one-line summary of their progress.
    pub(crate) progress_summaries: bool,
    /// Whether the text and thinking of subagents are forwarded, not only their tool calls.
    pub(crate) forward_subagent_text: bool,
    /// Whether the session connects to Remote Control as it starts; `None` follows the user's Claude settings.
    pub remote_control: Option<bool>,
    /// The longest a control request waits for its answer, where that is shorter than the request's own timeout
    /// (`control::Request::timeout`); for tests of a CLI that does not answer.
    pub control_timeout: Option<Duration>,
    /// Whether the workspace is trusted (E16); an untrusted one loads only the user's settings and no MCP server, so
    /// nothing from the project folder configures or runs.
    pub trusted: bool,
}

/// The variable that makes the CLI load the `CLAUDE.md` files of the folders `--add-dir` names.
const ADDED_DIRECTORIES_CLAUDE_MD: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";

/// The variable the CLI reads to know who started it; it is left for the CLI to choose, so the app never passes
/// itself off as another client such as the VS Code extension.
const ENTRYPOINT: &str = "CLAUDE_CODE_ENTRYPOINT";

impl Launch {
    /// A launch of `executable` in `cwd` with `env`, asking before tools run.
    pub fn new(executable: impl Into<PathBuf>, env: Vec<(String, String)>, cwd: impl Into<PathBuf>) -> Launch {
        Launch {
            executable: executable.into(),
            env,
            cwd: cwd.into(),
            model: None,
            permission_mode: Some("default".into()),
            effort: None,
            thinking: Thinking::Default,
            thinking_display: Some("summarized".into()),
            resume: None,
            session_id: None,
            resume_at: None,
            resume_drops_turn: None,
            fork_session: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            add_dirs: Vec::new(),
            allow_bypass: false,
            sandbox: false,
            persist: true,
            file_checkpoints: false,
            progress_summaries: true,
            forward_subagent_text: true,
            remote_control: None,
            control_timeout: None,
            trusted: false,
        }
    }

    /// Resumes `session` as a new branch that ends just before the prompt `rewind` names, guarded so that the CLI
    /// refuses rather than drops more than that prompt's turn.
    pub fn rewind(&mut self, session: &str, rewind: &Rewind) {
        self.resume = Some(session.to_string());
        self.resume_at = Some(rewind.fork_at.clone());
        self.resume_drops_turn = Some(rewind.drops_turn.clone());
        self.fork_session = true;
    }

    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "--print",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "--include-hook-events",
            "--permission-prompt-tool=stdio",
            // Prompts typed on claude.ai through Remote Control reach the app only as echoes.
            "--replay-user-messages",
        ]
        .map(str::to_string)
        .to_vec();
        // The `=` form keeps a value that starts with a dash from being read as a flag.
        let mut flag = |name: &str, value: &str| args.push(format!("--{name}={value}"));
        if let Some(model) = &self.model {
            flag("model", model);
        }
        if let Some(mode) = &self.permission_mode {
            flag("permission-mode", mode);
        }
        if let Some(effort) = &self.effort {
            flag("effort", effort);
        }
        match self.thinking {
            Thinking::Default => {}
            Thinking::Adaptive => flag("thinking", "adaptive"),
            Thinking::Disabled => flag("thinking", "disabled"),
            Thinking::Budget(tokens) => flag("max-thinking-tokens", &tokens.to_string()),
        }
        if let Some(display) = self
            .thinking_display
            .as_ref()
            .filter(|_| self.thinking != Thinking::Disabled)
        {
            flag("thinking-display", display);
        }
        if let Some(session) = &self.resume {
            flag("resume", session);
            if let Some(message) = &self.resume_at {
                flag("resume-session-at", message);
                if let Some(prompt) = &self.resume_drops_turn {
                    flag("resume-drops-turn", prompt);
                }
            }
        } else if let Some(id) = &self.session_id {
            flag("session-id", id);
        }
        if !self.trusted {
            flag("setting-sources", "user");
        }
        self.rules(&mut args);
        if self.allow_bypass {
            args.push("--allow-dangerously-skip-permissions".into());
        }
        if !self.persist {
            args.push("--no-session-persistence".into());
        }
        if self.fork_session && self.resume.is_some() {
            args.push("--fork-session".into());
        }
        args
    }

    /// The tool rules, the extra folders, the MCP servers and the settings.
    fn rules(&self, args: &mut Vec<String>) {
        if !self.allowed_tools.is_empty() {
            args.push(format!("--allowedTools={}", self.allowed_tools.join(",")));
        }
        if !self.disallowed_tools.is_empty() {
            args.push(format!("--disallowedTools={}", self.disallowed_tools.join(",")));
        }
        for dir in &self.add_dirs {
            args.push(format!("--add-dir={}", dir.to_string_lossy()));
        }
        if !self.trusted {
            // Without --mcp-config this loads no MCP server at all, the project's `.mcp.json` included.
            args.push("--strict-mcp-config".into());
        }
        if self.sandbox {
            // The CLI's sandbox covers shell commands only; failIfUnavailable keeps it from silently running without.
            args.push(r#"--settings={"sandbox":{"enabled":true,"failIfUnavailable":true}}"#.into());
        }
    }

    /// The environment: the login shell's, without a client name, with file backups when asked for.
    pub(crate) fn environment(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = self
            .env
            .iter()
            .filter(|(name, _)| {
                name != ENTRYPOINT
                    && name != "CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING"
                    && name != ADDED_DIRECTORIES_CLAUDE_MD
            })
            .cloned()
            .collect();
        if self.file_checkpoints {
            env.push(("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING".into(), "true".into()));
        }
        env
    }

    /// The command, with all three pipes, ready for the supervisor.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .args(self.arguments())
            .env_clear()
            .envs(self.environment())
            .current_dir(&self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch::new(
            "/usr/local/bin/claude",
            vec![
                ("PATH".into(), "/usr/bin".into()),
                (ENTRYPOINT.into(), "claude-vscode".into()),
            ],
            "/work",
        )
    }

    #[test]
    fn a_session_speaks_stream_json_both_ways_and_asks_through_stdio() {
        let args = launch().arguments();
        for expected in [
            "--print",
            "--input-format=stream-json",
            "--output-format=stream-json",
            "--verbose",
            "--include-partial-messages",
            "--include-hook-events",
            "--permission-prompt-tool=stdio",
            "--replay-user-messages",
            "--permission-mode=default",
            "--thinking-display=summarized",
        ] {
            assert!(
                args.contains(&expected.to_string()),
                "{expected} is missing from {args:?}"
            );
        }
        assert!(
            !args.iter().any(|a| a.starts_with("--settings")),
            "the sandbox is off by default"
        );
        assert!(
            !args.iter().any(|a| a == "--no-session-persistence"),
            "transcripts are kept for resuming"
        );
        assert!(!args.iter().any(|a| a.starts_with("--resume") || a == "--fork-session"));
        assert!(!args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn bypassing_permissions_is_allowed_without_starting_in_it() {
        let mut launch = launch();
        launch.allow_bypass = true;
        let args = launch.arguments();
        assert!(args.contains(&"--allow-dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--permission-mode=default".to_string()));
        assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn options_become_flags_with_their_values_attached() {
        let mut launch = launch();
        launch.model = Some("opus".into());
        launch.effort = Some("high".into());
        launch.thinking = Thinking::Budget(8000);
        launch.resume = Some("-odd-id".into());
        launch.allowed_tools = vec!["Read".into(), "Bash(git status:*)".into()];
        launch.disallowed_tools = vec!["WebFetch".into()];
        launch.sandbox = true;
        let args = launch.arguments();
        for expected in [
            "--model=opus",
            "--effort=high",
            "--max-thinking-tokens=8000",
            "--resume=-odd-id",
            "--allowedTools=Read,Bash(git status:*)",
            "--disallowedTools=WebFetch",
        ] {
            assert!(
                args.contains(&expected.to_string()),
                "{expected} is missing from {args:?}"
            );
        }
        let settings = args.iter().find(|a| a.starts_with("--settings=")).unwrap();
        let json: serde_json::Value = serde_json::from_str(settings.trim_start_matches("--settings=")).unwrap();
        assert_eq!(json["sandbox"]["enabled"], true);
    }

    #[test]
    fn a_session_can_be_resumed_up_to_a_message_as_a_branch() {
        let mut launch = launch();
        launch.resume_at = Some("m-1".into());
        launch.fork_session = true;
        let args = launch.arguments();
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("--resume-session-at") || a == "--fork-session"),
            "both only apply to a resumed session"
        );
        launch.resume = Some("s-1".into());
        let args = launch.arguments();
        for expected in ["--resume=s-1", "--resume-session-at=m-1", "--fork-session"] {
            assert!(
                args.contains(&expected.to_string()),
                "{expected} is missing from {args:?}"
            );
        }
    }

    #[test]
    fn a_rewind_resumes_a_branch_guarded_by_the_dropped_turn() {
        let mut launch = launch();
        launch.resume_drops_turn = Some("p-2".into());
        assert!(!launch.arguments().iter().any(|a| a.starts_with("--resume-drops-turn")));
        launch.rewind(
            "s-1",
            &Rewind {
                fork_at: "a-1".into(),
                drops_turn: "p-2".into(),
            },
        );
        let args = launch.arguments();
        for expected in [
            "--resume=s-1",
            "--resume-session-at=a-1",
            "--resume-drops-turn=p-2",
            "--fork-session",
        ] {
            assert!(
                args.contains(&expected.to_string()),
                "{expected} is missing from {args:?}"
            );
        }
        launch.resume_at = None;
        assert!(
            !launch.arguments().iter().any(|a| a.starts_with("--resume-drops-turn")),
            "the guard only applies to a resume up to a message"
        );
    }

    fn value_of<'a>(args: &'a [String], name: &str) -> &'a str {
        args.iter()
            .find_map(|a| a.strip_prefix(&format!("--{name}=")))
            .unwrap_or_else(|| panic!("--{name} is missing from {args:?}"))
    }

    #[test]
    fn an_untrusted_workspace_loads_only_the_users_settings_and_no_mcp_server() {
        let mut launch = launch();
        launch.permission_mode = Some("acceptEdits".into());
        let args = launch.arguments();
        assert_eq!(value_of(&args, "setting-sources"), "user");
        assert!(args.contains(&"--strict-mcp-config".to_string()), "{args:?}");
        assert!(!args.iter().any(|a| a.starts_with("--mcp-config")), "{args:?}");
        assert_eq!(
            value_of(&args, "permission-mode"),
            "acceptEdits",
            "the user's mode stays"
        );
        launch.trusted = true;
        let args = launch.arguments();
        assert!(!args.iter().any(|a| a.starts_with("--setting-sources")), "{args:?}");
        assert!(!args.contains(&"--strict-mcp-config".to_string()), "{args:?}");
    }

    #[test]
    fn a_new_session_takes_the_id_it_is_given_and_a_resumed_one_keeps_its_own() {
        let mut launch = launch();
        launch.session_id = Some("7f1c2a7e-0b4d-4c1e-9a55-3f6d2b8e9c10".into());
        assert_eq!(
            value_of(&launch.arguments(), "session-id"),
            "7f1c2a7e-0b4d-4c1e-9a55-3f6d2b8e9c10"
        );
        launch.resume = Some("s-1".into());
        launch.trusted = true;
        let args = launch.arguments();
        assert!(!args.iter().any(|a| a.starts_with("--session-id")), "{args:?}");
        assert!(
            !args.iter().any(|a| a.starts_with("--setting-sources")),
            "a trusted workspace loads every source"
        );
    }

    #[test]
    fn the_app_never_names_itself_as_another_client() {
        let mut launch = launch();
        assert!(launch.environment().iter().all(|(name, _)| name != ENTRYPOINT));
        launch.file_checkpoints = true;
        assert!(
            launch
                .environment()
                .contains(&("CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING".into(), "true".into()))
        );
        let command = launch.command();
        assert!(command.get_envs().all(|(name, _)| name != ENTRYPOINT));
        assert_eq!(command.get_current_dir(), Some(std::path::Path::new("/work")));
    }
}
