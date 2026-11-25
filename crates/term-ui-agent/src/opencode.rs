//! OpenCode through its own v2 HTTP API ("SDK first, ACP as fallback"): the OpenCode choice starts
//! `opencode serve` when the installed OpenCode is a release this build speaks (`term_opencode::TESTED` or newer), and
//! `opencode acp` otherwise. Its agents show as modes and its models and their variants as the composer's choices,
//! as an ACP agent's do.

use std::path::Path;

use splat_i18n::{t, tf};
use term_acp::capabilities::{Category, ConfigOption, Mode, OptionChoice, OptionValue};
use term_agents::{AgentPath, Launch, LaunchError, UserChoices};
#[cfg(not(feature = "selftest"))]
use term_process::Supervisor;
use term_thread::Thread;

use term_ui::workspace_host::Trust;

use crate::driven::Driven;
use crate::source::Problem;
use crate::tab::AgentTab;

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::set_test_opencode;

/// The id of the preset whose agent OpenCode is.
pub(crate) const PRESET: &str = "opencode";

/// How the OpenCode choice starts: its server when the program the preset finds on the PATH outside `cwd` speaks the
/// v2 API (E16); none to take `opencode acp`. This runs `opencode --version`, so it runs off the main thread.
pub(crate) fn launch_for(env: &[(String, String)], cwd: &Path) -> Option<term_opencode::Launch> {
    // On another machine OpenCode's server would need a port forwarded and its password there; ACP needs neither (E25).
    if term_remote::registry::bin_for(cwd).is_some() {
        return None;
    }
    #[cfg(feature = "selftest")]
    {
        let _ = env;
        selftest_probe::test_launch(cwd)
    }
    #[cfg(not(feature = "selftest"))]
    {
        let installs = crate::acp_install::installs();
        let program = term_acp::register::preset(PRESET)?.program(env, cwd, &installs)?;
        let version = term_opencode::version::probe(Supervisor::global(), &program, env).ok()?;
        let mut launch = term_opencode::Launch::new(program, env.to_vec(), cwd);
        launch.version = Some(version);
        Some(launch)
    }
}

fn choice(value: String, name: String, description: Option<String>) -> OptionChoice {
    OptionChoice {
        value,
        name,
        description,
        group: None,
    }
}

/// OpenCode's settings as an ACP agent's: its agents as modes, the model and the model's variants as options.
pub(crate) fn capabilities(caps: &term_opencode::Capabilities) -> term_acp::Capabilities {
    let mut acp = term_acp::Capabilities {
        agent: Some((
            term_opencode::AGENT.to_string(),
            Some("OpenCode".to_string()),
            caps.server_version.clone().unwrap_or_default(),
        )),
        load_session: true,
        images: caps.images(),
        modes: caps
            .modes()
            .into_iter()
            .map(|agent| Mode {
                id: agent.id.clone(),
                name: if agent.name.is_empty() {
                    agent.id.clone()
                } else {
                    agent.name.clone()
                },
                description: agent.description.clone(),
            })
            .collect(),
        mode: caps.mode().map(str::to_string),
        ..term_acp::Capabilities::default()
    };
    let models: Vec<OptionChoice> = caps
        .models
        .iter()
        .filter(|m| m.enabled)
        .map(|m| {
            let id = format!("{}/{}", m.provider, m.model_id);
            choice(id.clone(), if m.name.is_empty() { id } else { m.name.clone() }, None)
        })
        .collect();
    if !models.is_empty() {
        let current = caps
            .effective_model()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .unwrap_or_default();
        acp.options.push(ConfigOption {
            id: "model".into(),
            name: t("Modell").into(),
            description: None,
            category: Category::Model,
            value: OptionValue::Select {
                current,
                choices: models,
            },
        });
    }
    if !caps.variants().is_empty() {
        // Without a variant OpenCode takes the model's own default, which the picker offers by name.
        let variants: Vec<OptionChoice> = std::iter::once(choice(String::new(), t("Standard").into(), None))
            .chain(caps.variants().iter().map(|v| choice(v.clone(), v.clone(), None)))
            .collect();
        acp.options.push(ConfigOption {
            id: "effort".into(),
            name: t("Aufwand").into(),
            description: None,
            category: Category::ThoughtLevel,
            value: OptionValue::Select {
                current: caps.variant().unwrap_or_default().to_string(),
                choices: variants,
            },
        });
    }
    acp
}

/// `error` in the user's words.
pub(crate) fn explained(error: &term_opencode::SessionError) -> String {
    match error {
        term_opencode::SessionError::SignIn => {
            t("OpenCode hat noch kein Modell. Verbinde einen Anbieter mit „opencode auth login“ im Terminal.")
                .to_string()
        }
        term_opencode::SessionError::Unsupported { tested, .. } => {
            tf("Diese Version von OpenCode ist älter als {}.", &[&tested.to_string()])
        }
        other => tf("OpenCode: {}", &[&other.to_string()]),
    }
}

/// Why the OpenCode session did not start, as `explained` words OpenCode's own error.
fn start_problem(error: LaunchError) -> String {
    match error {
        LaunchError::NotFound(program) => explained(&term_opencode::SessionError::NotFound(program)),
        LaunchError::Start(why) => tf("OpenCode: {}", &[&why]),
        other => tf("OpenCode: {}", &[&other.to_string()]),
    }
}

impl AgentTab {
    /// Starts OpenCode's server as `launch` says for `thread` in the app's registry, continuing the thread's OpenCode
    /// session where it has one; in an untrusted workspace without the project's configuration (E16).
    pub(crate) fn launch_opencode(&self, launch: term_opencode::Launch, thread: Thread) {
        let resume = self.sdk_resume(&thread);
        let Some(sessions) = self.sessions.clone() else {
            return self.start_failed(Problem::Start(start_problem(LaunchError::Start(String::new()))));
        };
        let path = AgentPath::OpenCode {
            program: launch.program,
            version: launch.version,
        };
        let mut start = Launch::new(path, launch.cwd, launch.env, UserChoices::default());
        start.resume = resume;
        start.thread = Some(thread);
        start.trusted = self.controller.is_trusted();
        match Driven::start(&sessions, start) {
            Ok((driven, first)) => {
                self.run_session(driven);
                if let Some(first) = first {
                    self.absorb(first.thread, &[term_thread::Change::Cleared]);
                }
            }
            Err(error) => self.start_failed(crate::acp::launch_problem(error, start_problem)),
        }
    }
}
