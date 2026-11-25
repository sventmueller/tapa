//! The agent tabs (E21): the size of their text, which agents new tabs offer, and the mode, model and effort each
//! agent's new sessions start with, unless the user picked otherwise in a tab.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The agent tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// The size of the conversation's text in points.
    pub font_size: f64,
    /// Claude's sessions.
    pub claude: Claude,
    /// Codex's sessions.
    pub codex: Codex,
    /// OpenCode's sessions.
    pub opencode: OpenCode,
    /// Whether new tabs offer an agent, by the id its threads name; an agent not listed keeps what the app gives it.
    pub offer: BTreeMap<String, bool>,
}

impl Default for Agent {
    fn default() -> Agent {
        Agent {
            font_size: 13.0,
            claude: Claude::default(),
            codex: Codex::default(),
            opencode: OpenCode::default(),
            offer: BTreeMap::new(),
        }
    }
}

impl Agent {
    /// Whether new tabs offer the agent `id`, which they do by `default` unless the settings say otherwise.
    pub fn offers(&self, id: &str, default: bool) -> bool {
        self.offer.get(id).copied().unwrap_or(default)
    }
}

/// Claude's sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Claude {
    /// What a new session runs without asking.
    pub permission_mode: ClaudeMode,
    /// The model a new session starts with, by alias or full name; empty leaves it to Claude's own settings.
    pub model: String,
    /// How much a new session's model thinks, such as `high`; empty leaves it to the model.
    pub effort: String,
}

/// Claude's permission modes, by their names in Claude's settings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaudeMode {
    /// Asks for everything the rules do not allow.
    #[default]
    #[serde(rename = "default")]
    Ask,
    /// Takes edits without asking.
    #[serde(rename = "acceptEdits")]
    AcceptEdits,
    /// Only plans; changes nothing.
    #[serde(rename = "plan")]
    Plan,
}

impl ClaudeMode {
    /// Every mode, in the order the settings list them.
    pub const ALL: [ClaudeMode; 3] = [ClaudeMode::Ask, ClaudeMode::AcceptEdits, ClaudeMode::Plan];

    /// The mode's name as Claude's CLI takes it.
    pub fn id(self) -> &'static str {
        match self {
            ClaudeMode::Ask => "default",
            ClaudeMode::AcceptEdits => "acceptEdits",
            ClaudeMode::Plan => "plan",
        }
    }
}

/// Codex's sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Codex {
    /// What a new session may change without asking.
    pub approval: CodexApproval,
    /// The model a new session starts with; empty leaves it to Codex's own settings.
    pub model: String,
    /// How much a new session's model reasons, such as `medium`; empty leaves it to the model.
    pub effort: String,
}

/// OpenCode's sessions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenCode {
    /// The model a new session starts with, as `provider/model`; empty leaves it to OpenCode's own settings.
    pub model: String,
}

/// Codex's approval presets, by the ids Codex gives them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodexApproval {
    /// Reads, and asks before changing anything.
    ReadOnly,
    /// Works in the project and asks before going beyond it.
    #[default]
    Auto,
    /// Changes anything without asking.
    FullAccess,
}

impl CodexApproval {
    /// Every preset, in the order the settings list them.
    pub const ALL: [CodexApproval; 3] = [CodexApproval::ReadOnly, CodexApproval::Auto, CodexApproval::FullAccess];

    /// The preset's id as Codex names it.
    pub fn id(self) -> &'static str {
        match self {
            CodexApproval::ReadOnly => "read-only",
            CodexApproval::Auto => "auto",
            CodexApproval::FullAccess => "full-access",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_section_reads_the_names_the_agents_use() {
        let text = "version = 1\n[agent]\nfont_size = 15\n[agent.claude]\npermission_mode = \"acceptEdits\"\n[agent.codex]\napproval = \"read-only\"\n";
        let (config, warnings) = crate::Config::parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.agent.font_size, 15.0);
        assert_eq!(config.agent.claude.permission_mode, ClaudeMode::AcceptEdits);
        assert_eq!(config.agent.codex.approval, CodexApproval::ReadOnly);
        assert_eq!(config.agent.codex.approval.id(), "read-only");
        let (plain, _) = crate::Config::parse("version = 1\n");
        assert_eq!(plain.agent, Agent::default());
        assert_eq!(plain.agent.claude.permission_mode.id(), "default");
    }

    #[test]
    fn models_efforts_and_offers_are_read_and_default_to_the_agents_own() {
        let text = "version = 1\n[agent.claude]\nmodel = \"opus\"\neffort = \"high\"\n[agent.codex]\nmodel = \"gpt-x\"\n[agent.opencode]\nmodel = \"anthropic/claude\"\n[agent.offer]\ngemini = true\nopencode = false\n";
        let (config, warnings) = crate::Config::parse(text);
        assert!(warnings.is_empty(), "{warnings:?}");
        let agent = &config.agent;
        assert_eq!(
            (agent.claude.model.as_str(), agent.claude.effort.as_str()),
            ("opus", "high")
        );
        assert_eq!((agent.codex.model.as_str(), agent.codex.effort.as_str()), ("gpt-x", ""));
        assert_eq!(agent.opencode.model, "anthropic/claude");
        assert!(agent.offers("gemini", false));
        assert!(!agent.offers("opencode", true));
        assert!(agent.offers("claude", true));
        assert!(!agent.offers("antigravity-acp", false));
        let plain = Agent::default();
        assert!(plain.claude.model.is_empty() && plain.codex.effort.is_empty() && plain.offer.is_empty());
    }

    #[test]
    fn a_mode_no_agent_knows_keeps_the_default() {
        let (config, warnings) = crate::Config::parse("version = 1\n[agent.claude]\npermission_mode = \"yolo\"\n");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(config.agent, Agent::default());
    }
}
