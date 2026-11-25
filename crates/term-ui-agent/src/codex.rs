//! Codex through its own app-server protocol ("SDK first, ACP as fallback"): the Codex choice starts
//! `codex app-server` when the installed Codex is a release this build speaks (`term_codex::TESTED` or newer), and
//! the `codex-acp` adapter on the PATH otherwise. The session fills the same thread model, and its settings show in the composer
//! as an ACP agent's do, under the names `term_codex` gives them.

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
pub use selftest_probe::set_test_codex;

/// The id of the preset whose agent Codex is.
pub(crate) const PRESET: &str = "codex-acp";

/// How the Codex choice starts in `cwd` with the login shell's `env`: the app-server when an installed Codex outside
/// `cwd` speaks its protocol (E16); none to take the ACP adapter. This runs Codex, so it runs off the main thread.
pub(crate) fn launch_for(env: &[(String, String)], cwd: &Path) -> Option<term_codex::Launch> {
    #[cfg(feature = "selftest")]
    {
        let _ = env;
        selftest_probe::test_launch(cwd)
    }
    #[cfg(not(feature = "selftest"))]
    {
        let program = term_process::tool::which_outside("codex", term_process::tool::var(env, "PATH"), cwd)?;
        let version = term_codex::version::probe(Supervisor::global(), &program, env).ok()?;
        let mut launch = term_codex::Launch::new(program, env.to_vec(), cwd);
        launch.version = Some(version);
        Some(launch)
    }
}

/// The file that keeps the Codex threads of each folder as `thread/list` last named them, for the session list.
fn history_file() -> std::path::PathBuf {
    term_base::data_dir().join("codex-threads.json")
}

/// Asks the running Codex for the stored threads of `cwd`, also those begun outside the app, and keeps them for the
/// session list, which lists them as sessions to continue.
pub(crate) fn remember_threads(session: &term_codex::Session, cwd: &Path) {
    let query = term_codex::ThreadQuery {
        cwd: Some(cwd.to_path_buf()),
        limit: Some(50),
        ..term_codex::ThreadQuery::default()
    };
    let folder = cwd.to_string_lossy().into_owned();
    session.threads(&query).then(move |page| {
        let Ok(page) = page else { return };
        let threads: Vec<serde_json::Value> = page
            .threads
            .iter()
            .map(|t| {
                let title = t
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| t.preview.clone());
                serde_json::json!({"id": t.id, "title": title, "updated": t.updated_at})
            })
            .collect();
        let file = history_file();
        let mut all: serde_json::Map<String, serde_json::Value> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        all.insert(folder, serde_json::Value::from(threads));
        let bytes = serde_json::to_vec(&all).unwrap_or_default();
        if let Err(error) = term_base::atomic_write::write(&file, &bytes) {
            crate::common::diag::report("keeping the Codex threads", error);
        }
    });
}

/// The Codex threads of `cwd` the running Codex last named, newest first, as sessions to continue.
pub(crate) fn past_threads(cwd: &Path) -> Vec<(crate::Resume, std::time::SystemTime)> {
    let all: serde_json::Value = std::fs::read(history_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    all[cwd.to_string_lossy().as_ref()]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let updated = u64::try_from(t["updated"].as_i64()?).ok()?;
            Some((
                crate::Resume {
                    session: t["id"].as_str()?.to_string(),
                    thread: None,
                    title: t["title"].as_str().map(str::to_string).filter(|s| !s.is_empty()),
                    transcript: None,
                    agent: Some(PRESET.to_string()),
                },
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(updated),
            ))
        })
        .collect()
}

/// Codex's settings as an ACP agent's: its approval presets as modes, the model and the model's efforts as options.
pub(crate) fn capabilities(caps: &term_codex::Capabilities) -> term_acp::Capabilities {
    let modes = term_codex::MODES
        .iter()
        .map(|mode| Mode {
            id: mode.id.to_string(),
            name: mode_name(mode.id).to_string(),
            description: None,
        })
        .collect();
    let mut options = Vec::new();
    let models: Vec<OptionChoice> = caps
        .models
        .iter()
        .filter(|m| !m.hidden)
        .map(|m| OptionChoice {
            value: m.model.clone(),
            name: if m.display_name.is_empty() {
                m.model.clone()
            } else {
                m.display_name.clone()
            },
            description: (!m.description.is_empty()).then(|| m.description.clone()),
            group: None,
        })
        .collect();
    if !models.is_empty() {
        options.push(ConfigOption {
            id: "model".into(),
            name: t("Modell").into(),
            description: None,
            category: Category::Model,
            value: OptionValue::Select {
                current: caps.model.clone().unwrap_or_default(),
                choices: models,
            },
        });
    }
    let efforts: Vec<OptionChoice> = caps
        .efforts()
        .iter()
        .map(|e| OptionChoice {
            value: e.reasoning_effort.clone(),
            name: e.reasoning_effort.clone(),
            description: (!e.description.is_empty()).then(|| e.description.clone()),
            group: None,
        })
        .collect();
    if !efforts.is_empty() {
        options.push(ConfigOption {
            id: "effort".into(),
            name: t("Aufwand").into(),
            description: None,
            category: Category::ThoughtLevel,
            value: OptionValue::Select {
                current: caps.effort.clone().unwrap_or_default(),
                choices: efforts,
            },
        });
    }
    term_acp::Capabilities {
        agent: Some((
            term_codex::AGENT.to_string(),
            Some("Codex".to_string()),
            caps.version.map(|v| v.to_string()).unwrap_or_default(),
        )),
        load_session: true,
        images: caps.images(),
        modes,
        mode: caps.mode().map(str::to_string),
        options,
        ..term_acp::Capabilities::default()
    }
}

fn mode_name(id: &str) -> &'static str {
    match id {
        "read-only" => t("Nur lesen"),
        "auto" => t("Im Projekt arbeiten"),
        _ => t("Voller Zugriff"),
    }
}

/// `error` in the user's words.
pub(crate) fn explained(error: &term_codex::SessionError) -> String {
    match error {
        term_codex::SessionError::SignIn => {
            t("Codex braucht eine Anmeldung. Melde dich im Terminal mit „codex login“ an.").to_string()
        }
        term_codex::SessionError::Unsupported { tested, .. } => {
            tf("Diese Version von Codex ist älter als {}.", &[&tested.to_string()])
        }
        other => tf("Codex: {}", &[&other.to_string()]),
    }
}

/// Why the Codex session did not start, as `explained` words Codex's own error.
fn start_problem(error: LaunchError) -> String {
    match error {
        LaunchError::NotFound(program) => explained(&term_codex::SessionError::NotFound(program)),
        LaunchError::Start(why) => tf("Codex: {}", &[&why]),
        other => tf("Codex: {}", &[&other.to_string()]),
    }
}

impl AgentTab {
    /// Starts Codex's app-server as `launch` says for `thread` in the app's registry, continuing the thread's Codex
    /// thread where it has one; in an untrusted workspace without the project's configuration (E16).
    pub(crate) fn launch_codex(&self, launch: term_codex::Launch, thread: Thread) {
        let resume = self.sdk_resume(&thread);
        let Some(sessions) = self.sessions.clone() else {
            return self.start_failed(Problem::Start(start_problem(LaunchError::Start(String::new()))));
        };
        let path = AgentPath::Codex {
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
