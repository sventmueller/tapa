//! What a session starts with: the path to the agent, the folder, the environment, the model, the session to resume,
//! the user's choices and whether the workspace is trusted (E16).

use std::path::PathBuf;

use term_thread::Thread;

/// The four paths to an agent, and the scripted session of the tests.
pub enum AgentPath {
    /// Claude Code in stream-json mode (`term-claude`).
    Claude {
        /// The `claude` executable.
        program: PathBuf,
        /// Whether the session connects to Remote Control as it starts; `None` follows the user's Claude settings.
        remote_control: Option<bool>,
        /// Where the session `Launch::resume` names branches off: it continues as a new session that ends just
        /// before a prompt, guarded so that the CLI refuses rather than drops more than that prompt's turn (I6).
        rewind: Option<term_thread::Rewind>,
    },
    /// Codex over its app server (`term-codex`).
    Codex {
        /// The `codex` executable.
        program: PathBuf,
        /// The release already read; the handshake reads it when `None`.
        version: Option<term_codex::Version>,
        /// The app prepared the shared daemon for Remote Control.
        shared: bool,
    },
    /// OpenCode over its HTTP API (`term-opencode`).
    OpenCode {
        /// The `opencode` executable.
        program: PathBuf,
        /// The release already read; the handshake reads it when `None`.
        version: Option<term_opencode::Version>,
    },
    /// An ACP agent, as the app resolved its preset from the PATH (`term-acp`).
    Acp(Box<term_acp::Launch>),
    /// A session that plays a script, for the self-test and the tests (F61).
    #[cfg(any(test, feature = "selftest"))]
    Scripted(crate::script::Script),
    /// A recording of Claude Code played into its thread by the caller, with no process behind it, for the
    /// self-test's AI tabs; it takes no prompts.
    #[cfg(any(test, feature = "selftest"))]
    Replay(Box<term_claude::replay::Replay>),
}

/// Which path a session takes, without what it needs to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// OpenCode.
    OpenCode,
    /// An ACP agent.
    Acp,
    /// A scripted session.
    Scripted,
    /// A played recording of Claude Code.
    Replay,
}

impl AgentPath {
    /// Claude Code's `program`, as the user's Claude settings set Remote Control, with no rewind.
    pub fn claude(program: impl Into<PathBuf>) -> AgentPath {
        AgentPath::Claude {
            program: program.into(),
            remote_control: None,
            rewind: None,
        }
    }

    /// Which path it is.
    pub fn kind(&self) -> PathKind {
        match self {
            AgentPath::Claude { .. } => PathKind::Claude,
            AgentPath::Codex { .. } => PathKind::Codex,
            AgentPath::OpenCode { .. } => PathKind::OpenCode,
            AgentPath::Acp(_) => PathKind::Acp,
            #[cfg(any(test, feature = "selftest"))]
            AgentPath::Scripted(_) => PathKind::Scripted,
            #[cfg(any(test, feature = "selftest"))]
            AgentPath::Replay(_) => PathKind::Replay,
        }
    }
}

/// The choices of a session the user drives, as the AI tab makes them today.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserChoices {
    /// The mode: Claude's permission mode, a Codex preset of `term_codex::MODES`, an OpenCode agent; the path's own
    /// when `None`.
    pub mode: Option<String>,
    /// Whether Claude's shell commands run in its sandbox.
    pub sandbox: bool,
}

/// Everything that decides how a session starts.
pub struct Launch {
    /// The path to the agent.
    pub path: AgentPath,
    /// The folder the agent works in, the workspace's base path.
    pub folder: PathBuf,
    /// The environment the agent gets: the login shell's.
    pub env: Vec<(String, String)>,
    /// The model; the path's default when `None`.
    pub model: Option<String>,
    /// The reasoning effort; the path's default when `None`.
    pub effort: Option<String>,
    /// The agent's id of a session to resume.
    pub resume: Option<String>,
    /// The id a new session takes, where the path lets the caller choose it at the start (Claude: a UUID), so it can
    /// be resumed before its first turn named it.
    pub session_id: Option<String>,
    /// The thread to fill, such as one rebuilt from a transcript; a new one when `None`.
    pub thread: Option<Thread>,
    /// Whether the workspace is trusted (E16); an untrusted one starts the agent without the project's
    /// configuration, hooks and MCP servers, or not at all where the agent cannot skip them.
    pub trusted: bool,
    /// The user's choices.
    pub choices: UserChoices,
}

impl Launch {
    /// A launch of `path` in `folder` with `env` and `choices`, with the path's own model, in an untrusted workspace.
    pub fn new(
        path: AgentPath,
        folder: impl Into<PathBuf>,
        env: Vec<(String, String)>,
        choices: UserChoices,
    ) -> Launch {
        Launch {
            path,
            folder: folder.into(),
            env,
            model: None,
            effort: None,
            resume: None,
            session_id: None,
            thread: None,
            trusted: false,
            choices,
        }
    }
}

/// Why a session did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The agent cannot skip the project's configuration and the workspace is not trusted; the words for the user.
    Untrusted(String),
    /// The program is not there.
    NotFound(PathBuf),
    /// The session did not start; the path's own words.
    Start(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Untrusted(why) => f.write_str(why),
            LaunchError::NotFound(program) => write!(f, "{} is not there", program.display()),
            LaunchError::Start(why) => write!(f, "the session did not start: {why}"),
        }
    }
}

impl std::error::Error for LaunchError {}
