//! What the app keeps for the agent tabs beside the sealed restore: what the user picked last for each agent and the
//! folders where Claude stored permission rules, in `agents.json` in the data folder.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use term_ui_agent::AgentChoice;

const FILE: &str = "agents.json";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    choices: BTreeMap<String, AgentChoice>,
    claude_rule_folders: Vec<PathBuf>,
}

fn path() -> PathBuf {
    term_base::data_dir().join(FILE)
}

fn load() -> Stored {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(stored: &Stored) {
    if let Err(error) = term_base::atomic_write::write_json(&path(), stored) {
        term_base::diag::report("storing the agents' choices", &error);
    }
}

/// What the user picked last for agent `agent`; what they did not pick, the settings give.
pub(crate) fn choice(agent: &str, config: &term_config::Agent) -> AgentChoice {
    let mut choice = load().choices.remove(agent).unwrap_or_default();
    let defaults = defaults(agent, config);
    for (slot, default) in [
        (&mut choice.mode, defaults.mode),
        (&mut choice.model, defaults.model),
        (&mut choice.effort, defaults.effort),
    ] {
        if slot.is_none() {
            *slot = default;
        }
    }
    choice
}

/// Keeps `choice` as what the user picked last for agent `agent`; what the settings give anyway stays theirs, so
/// that changing the settings still reaches it.
pub(crate) fn remember(agent: &str, mut choice: AgentChoice, config: &term_config::Agent) {
    let defaults = defaults(agent, config);
    for (slot, default) in [
        (&mut choice.mode, defaults.mode),
        (&mut choice.model, defaults.model),
        (&mut choice.effort, defaults.effort),
    ] {
        if *slot == default {
            *slot = None;
        }
    }
    let mut stored = load();
    stored.choices.insert(agent.to_string(), choice);
    save(&stored);
}

/// The folders where Claude stored a permission rule from the app.
pub(crate) fn rule_folders() -> Vec<PathBuf> {
    load().claude_rule_folders
}

/// Notes `folder` as one where Claude stored a permission rule.
pub(crate) fn remember_rule_folder(folder: PathBuf) {
    let mut stored = load();
    if !stored.claude_rule_folders.contains(&folder) {
        stored.claude_rule_folders.push(folder);
        save(&stored);
    }
}

/// The mode, model and effort the settings give new sessions of agent `agent`; none where they leave it to the agent.
fn defaults(agent: &str, config: &term_config::Agent) -> AgentChoice {
    use term_ui_agent::Agent;
    let given = |text: &str| (!text.trim().is_empty()).then(|| text.trim().to_string());
    if agent == Agent::Claude.id() {
        AgentChoice {
            mode: Some(config.claude.permission_mode.id().to_string()),
            model: given(&config.claude.model),
            effort: given(&config.claude.effort),
        }
    } else if agent == Agent::Codex.id() {
        AgentChoice {
            mode: Some(config.codex.approval.id().to_string()),
            model: given(&config.codex.model),
            effort: given(&config.codex.effort),
        }
    } else if agent == Agent::OpenCode.id() {
        AgentChoice {
            model: given(&config.opencode.model),
            ..AgentChoice::default()
        }
    } else {
        AgentChoice::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_give_mode_model_and_effort_until_the_user_picks_them() {
        let mut config = term_config::Agent::default();
        config.codex.approval = term_config::CodexApproval::ReadOnly;
        config.claude.model = " opus ".into();
        config.opencode.model = "anthropic/claude".into();
        let (claude, codex, opencode) = (
            term_ui_agent::Agent::Claude,
            term_ui_agent::Agent::Codex,
            term_ui_agent::Agent::OpenCode,
        );
        let given = defaults(claude.id(), &config);
        assert_eq!(given.mode.as_deref(), Some("default"));
        assert_eq!(given.model.as_deref(), Some("opus"));
        assert_eq!(given.effort, None);
        assert_eq!(defaults(codex.id(), &config).mode.as_deref(), Some("read-only"));
        assert_eq!(defaults(codex.id(), &config).model, None);
        assert_eq!(
            defaults(opencode.id(), &config).model.as_deref(),
            Some("anthropic/claude")
        );
        assert_eq!(defaults("gemini", &config), AgentChoice::default());
    }
}
