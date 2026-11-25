//! The app as the agent tabs reach it beside the workspace (E3): its terminals, the files of a workspace, trust,
//! notifications and what the user picked last for each agent.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use term_ui::workspace_host::WorkspaceId;

/// A file of a workspace that matches what the user typed after `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The path relative to the workspace's base path, with `/` between folders.
    pub path: String,
}

/// The model, reasoning effort and permission mode the user picked for an agent, each as the agent names it; unset
/// leaves the agent's own default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentChoice {
    /// The model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How much the model thinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// What runs without asking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// What the agent tabs need of the app beside the workspace, which they reach through `term_ui::workspace_host`.
pub trait Host {
    /// The agent tabs' state in the running app.
    fn state(&self) -> Rc<crate::State>;
    /// The label and recent text of the terminal in view in `workspace`, else of the one opened last.
    fn recent_output(&self, workspace: WorkspaceId) -> Option<(String, String)>;
    /// The selection of an editor in `workspace`: its file, first and last line from 1, and text; a terminal has none.
    fn editor_selection(&self, workspace: WorkspaceId) -> Option<(PathBuf, (u32, u32), String)> {
        let _ = workspace;
        None
    }
    /// Looks for the files below the base path of `workspace` that match `query`, at most `limit`, off the main
    /// thread.
    fn query_files(&self, workspace: WorkspaceId, query: &str, limit: usize, answer: Box<dyn FnOnce(Vec<Hit>) + Send>);
    /// Opens a terminal tab of `workspace` in `cwd`.
    fn open_terminal(&self, workspace: WorkspaceId, cwd: &Path);
    /// Keeps the permission rules Claude's settings in `cwd` gained.
    fn remember_claude_rules(&self, cwd: &Path);
    /// A session finished a turn in `cwd`, so the sidebar reads the workspace's Git state again.
    fn turn_ended(&self, workspace: WorkspaceId, cwd: &Path);
    /// The user trusts `workspace` (E21).
    fn trust(&self, workspace: WorkspaceId);
    /// Tells the user who looks elsewhere that the tab `key` of `workspace` finished or waits, as a notification.
    fn post_notice(&self, workspace: WorkspaceId, key: &str, title: &str, body: &str);
    /// What the user picked last for the agent with the id `agent`.
    fn agent_choice(&self, agent: &str) -> AgentChoice;
    /// Keeps `choice` as what the user picked last for the agent with the id `agent`.
    fn remember_choice(&self, agent: &str, choice: AgentChoice);
}

static HOST: OnceLock<&'static (dyn Host + Sync)> = OnceLock::new();

/// Makes `host` the app the agent tabs belong to; once, before the first agent tab opens.
pub fn install(host: &'static (dyn Host + Sync)) {
    let _ = HOST.set(host);
}

/// The app the agent tabs belong to.
pub fn host() -> &'static (dyn Host + Sync) {
    *HOST
        .get()
        .expect("the app installs its host before the first agent tab")
}

/// The agent tabs' state in the running app.
pub fn state() -> Rc<crate::State> {
    host().state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_keeps_only_what_the_user_picked() {
        let picked = AgentChoice {
            model: Some("opus".into()),
            effort: None,
            mode: Some("plan".into()),
        };
        let text = serde_json::to_string(&picked).unwrap();
        assert_eq!(text, r#"{"model":"opus","mode":"plan"}"#);
        assert_eq!(serde_json::from_str::<AgentChoice>(&text).unwrap(), picked);
        assert_eq!(
            serde_json::from_str::<AgentChoice>("{}").unwrap(),
            AgentChoice::default()
        );
    }
}
