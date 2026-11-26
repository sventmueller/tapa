//! Starting a session on each path with the user's choices, as the agent tab makes them, and the workspace's trust
//! (E16): an untrusted workspace starts every path without the project's configuration, hooks and MCP servers.

use std::sync::Arc;

use term_base::explain::Explain;
use term_process::Supervisor;
use term_thread::{Thread, ThreadId};

use crate::backend::Backend;
use crate::inbox::Inbox;
use crate::launch::{AgentPath, Launch, LaunchError, UserChoices};

/// The program of `path` where `folder` is a remote workspace's and the program is not one of its stand-ins: it would
/// run on this Mac instead of the other machine (E25).
fn remote_program_elsewhere(path: &AgentPath, folder: &std::path::Path) -> Option<std::path::PathBuf> {
    let bin = term_remote::registry::bin_for(folder)?;
    let program = match path {
        AgentPath::Claude { program, .. } | AgentPath::Codex { program, .. } | AgentPath::OpenCode { program, .. } => {
            program.clone()
        }
        AgentPath::Acp(launch) => launch.program.clone(),
        #[cfg(any(test, feature = "selftest"))]
        _ => return None,
    };
    (!program.starts_with(&bin)).then_some(program)
}

/// Starts the session `launch` describes, reporting to `inbox`.
pub(crate) fn start(launch: Launch, supervisor: &Supervisor, inbox: &Arc<Inbox>) -> Result<Backend, LaunchError> {
    let Launch {
        path,
        folder,
        env,
        model,
        effort,
        resume,
        session_id,
        thread,
        trusted,
        choices,
    } = launch;
    if let Some(program) = remote_program_elsewhere(&path, &folder) {
        return Err(LaunchError::NotFound(program));
    }
    let thread = thread.unwrap_or_else(|| Thread::new(ThreadId::new(), agent_id(&path), &folder));
    let common = Common {
        folder,
        env,
        model,
        effort,
        resume,
        session_id,
        thread,
        trusted,
        choices,
    };
    match path {
        AgentPath::Claude {
            program,
            remote_control,
            rewind,
        } => claude(program, (remote_control, rewind), common, supervisor, inbox),
        AgentPath::Codex {
            program,
            version,
            shared,
        } => codex(program, version, shared, common, supervisor, inbox),
        AgentPath::OpenCode { program, version } => opencode(program, version, common, supervisor, inbox),
        AgentPath::Acp(launch) => acp(*launch, common, supervisor, inbox),
        #[cfg(any(test, feature = "selftest"))]
        AgentPath::Scripted(script) => Ok(Backend::Script(crate::script::Session::start(
            script,
            common.folder,
            common.thread,
            inbox.script(),
        ))),
        #[cfg(any(test, feature = "selftest"))]
        AgentPath::Replay(replay) => Ok(Backend::Replay(Box::new(std::cell::RefCell::new(*replay)))),
    }
}

/// What every path takes from the launch.
struct Common {
    folder: std::path::PathBuf,
    env: Vec<(String, String)>,
    model: Option<String>,
    effort: Option<String>,
    resume: Option<String>,
    session_id: Option<String>,
    thread: Thread,
    trusted: bool,
    choices: UserChoices,
}

/// The agent a new thread names, as the agent tab names it for the same path.
fn agent_id(path: &AgentPath) -> String {
    match path {
        AgentPath::Claude { .. } => term_claude::AGENT.into(),
        AgentPath::Codex { .. } => term_codex::AGENT.into(),
        AgentPath::OpenCode { .. } => term_opencode::AGENT.into(),
        AgentPath::Acp(launch) => launch.agent.clone(),
        #[cfg(any(test, feature = "selftest"))]
        AgentPath::Scripted(_) => "script".into(),
        #[cfg(any(test, feature = "selftest"))]
        AgentPath::Replay(_) => term_claude::AGENT.into(),
    }
}

/// The Claude launch of `common`.
fn claude_launch(
    program: std::path::PathBuf,
    (remote_control, rewind): (Option<bool>, Option<term_thread::Rewind>),
    common: &mut Common,
) -> term_claude::Launch {
    let mut launch = term_claude::Launch::new(program, std::mem::take(&mut common.env), &common.folder);
    launch.model = common.model.take();
    launch.effort = common.effort.take();
    launch.resume = common.resume.take();
    launch.session_id = common.session_id.take();
    launch.remote_control = remote_control;
    if let (Some(rewind), Some(session)) = (rewind, launch.resume.clone()) {
        launch.rewind(&session, &rewind);
    }
    launch.permission_mode = Some(common.choices.mode.clone().unwrap_or_else(|| "default".into()));
    launch.sandbox = common.choices.sandbox;
    launch.allow_bypass = true;
    launch.file_checkpoints = true;
    launch.trusted = common.trusted;
    launch
}

fn claude(
    program: std::path::PathBuf,
    claude: (Option<bool>, Option<term_thread::Rewind>),
    mut common: Common,
    supervisor: &Supervisor,
    inbox: &Arc<Inbox>,
) -> Result<Backend, LaunchError> {
    let launch = claude_launch(program, claude, &mut common);
    term_claude::Session::start(supervisor, &launch, common.thread, inbox.claude())
        .map(Backend::Claude)
        .map_err(|error| match error {
            term_claude::SessionError::NotFound(program) => LaunchError::NotFound(program),
            other => LaunchError::Start(other.to_string()),
        })
}

fn codex(
    program: std::path::PathBuf,
    version: Option<term_codex::Version>,
    shared: bool,
    common: Common,
    supervisor: &Supervisor,
    inbox: &Arc<Inbox>,
) -> Result<Backend, LaunchError> {
    let mut launch = term_codex::Launch::new(program, common.env, &common.folder);
    launch.version = version;
    launch.model = common.model;
    launch.effort = common.effort;
    launch.resume = common.resume;
    launch.trusted = common.trusted;
    launch.shared = shared;
    if let Some(mode) = term_codex::MODES
        .iter()
        .find(|m| Some(m.id) == common.choices.mode.as_deref())
    {
        launch.approval = Some(mode.approval);
        launch.sandbox = Some(mode.sandbox);
    }
    term_codex::Session::start(supervisor, launch, common.thread, inbox.codex())
        .map(Backend::Codex)
        .map_err(|error| match error {
            term_codex::SessionError::NotFound(program) => LaunchError::NotFound(program),
            other => LaunchError::Start(other.to_string()),
        })
}

fn opencode(
    program: std::path::PathBuf,
    version: Option<term_opencode::Version>,
    common: Common,
    supervisor: &Supervisor,
    inbox: &Arc<Inbox>,
) -> Result<Backend, LaunchError> {
    let mut launch = term_opencode::Launch::new(program, common.env, &common.folder);
    launch.version = version;
    launch.model = common.model;
    launch.resume = common.resume;
    launch.agent = common.choices.mode;
    launch.trusted = common.trusted;
    term_opencode::Session::start(supervisor, launch, common.thread, inbox.opencode())
        .map(Backend::OpenCode)
        .map_err(|error| match error {
            term_opencode::SessionError::NotFound(program) => LaunchError::NotFound(program),
            other => LaunchError::Start(other.to_string()),
        })
}

fn acp(
    mut launch: term_acp::Launch,
    common: Common,
    supervisor: &Supervisor,
    inbox: &Arc<Inbox>,
) -> Result<Backend, LaunchError> {
    launch.cwd = common.folder;
    launch.trusted = common.trusted;
    if common.resume.is_some() {
        launch.resume = common.resume;
    }
    term_acp::Session::start(supervisor, launch, common.thread, inbox.acp())
        .map(Backend::Acp)
        .map_err(|error| match error {
            term_acp::SessionError::NotFound(program) => LaunchError::NotFound(program),
            term_acp::SessionError::Start(why) => LaunchError::Start(why),
            error @ term_acp::SessionError::Untrusted(_) => LaunchError::Untrusted(error.explain()),
            other => LaunchError::Start(other.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn common(trusted: bool) -> Common {
        Common {
            folder: "/work".into(),
            env: Vec::new(),
            model: None,
            effort: None,
            resume: None,
            session_id: None,
            thread: Thread::new(ThreadId::from("t"), "claude", "/work"),
            trusted,
            choices: UserChoices {
                mode: Some("acceptEdits".into()),
                sandbox: false,
            },
        }
    }

    #[test]
    fn claude_takes_the_users_mode_and_the_workspaces_trust() {
        let launch = claude_launch("/bin/claude".into(), (None, None), &mut common(false));
        assert_eq!(launch.permission_mode.as_deref(), Some("acceptEdits"));
        assert!(!launch.trusted);
        assert!(claude_launch("/bin/claude".into(), (None, None), &mut common(true)).trusted);
    }
}
