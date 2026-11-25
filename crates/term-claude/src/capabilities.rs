//! What the running CLI offers, as `system/init` and the `initialize` answer report it, and which features the app
//! may promise.

use crate::control::{Account, CommandInfo, InitializeResult, ModelInfo};
use crate::protocol::Init;

/// How firmly a feature is promised (E21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Acceptance depends on it.
    Core,
    /// Best effort: built on parts the CLI marks experimental or keeps internal; the UI hides it when it fails.
    Experimental,
}

/// A feature of the Claude connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Prompts and streamed answers.
    Prompt,
    /// Thinking.
    Thinking,
    /// Tool cards.
    ToolCalls,
    /// Permission requests, with the input editable before allowing.
    Permissions,
    /// Stopping the turn.
    Interrupt,
    /// Changing model and permission mode.
    ModelAndMode,
    /// Subagents and their tool calls.
    Subagents,
    /// Background tasks, their output and stopping them.
    BackgroundTasks,
    /// Compaction notices.
    Compaction,
    /// Rate-limit events.
    RateLimits,
    /// Retry notices.
    Retries,
    /// Resuming a session by id.
    Resume,
    /// The plan's usage bars from `get_usage`.
    Usage,
    /// The context breakdown from `get_context_usage`.
    ContextBreakdown,
    /// Restoring files with `rewind_files`.
    RewindFiles,
    /// Hook output.
    HookOutput,
    /// Listing past sessions from `~/.claude/projects`.
    SessionList,
}

impl Feature {
    /// How firmly it is promised.
    pub fn tier(self) -> Tier {
        match self {
            Feature::Usage
            | Feature::ContextBreakdown
            | Feature::RewindFiles
            | Feature::HookOutput
            | Feature::SessionList => Tier::Experimental,
            _ => Tier::Core,
        }
    }
}

/// What the running CLI reported about itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Capabilities {
    /// The CLI's version.
    pub version: Option<String>,
    /// The tools the model can call.
    pub(crate) tools: Vec<String>,
    /// The slash commands, by name.
    pub slash_commands: Vec<String>,
    /// The slash commands with descriptions, from `initialize`.
    pub commands: Vec<CommandInfo>,
    /// The subagent types.
    pub(crate) agents: Vec<String>,
    pub(crate) skills: Vec<String>,
    /// The MCP servers with their state.
    pub(crate) mcp_servers: Vec<(String, String)>,
    /// The protocol capabilities, such as `interrupt_receipt_v1`.
    pub(crate) protocol: Vec<String>,
    /// The models the session can use.
    pub models: Vec<ModelInfo>,
    /// The signed-in account.
    pub account: Option<Account>,
    /// The output style in effect.
    pub output_style: Option<String>,
    /// The output styles the user can choose.
    pub output_styles: Vec<String>,
}

impl Capabilities {
    /// Takes in what a `system/init` frame reports.
    pub(crate) fn learn_init(&mut self, init: &Init) {
        if !init.claude_code_version.is_empty() {
            self.version = Some(init.claude_code_version.clone());
        }
        self.tools.clone_from(&init.tools);
        self.slash_commands.clone_from(&init.slash_commands);
        self.agents.clone_from(&init.agents);
        self.skills.clone_from(&init.skills);
        self.mcp_servers = init
            .mcp_servers
            .iter()
            .map(|s| (s.name.clone(), s.status.clone()))
            .collect();
        self.protocol.clone_from(&init.capabilities);
        if !init.output_style.is_empty() {
            self.output_style = Some(init.output_style.clone());
        }
    }

    /// Takes in what the `initialize` answer reports.
    pub(crate) fn learn_initialize(&mut self, result: &InitializeResult) {
        self.commands.clone_from(&result.commands);
        self.models.clone_from(&result.models);
        self.account = Some(result.account.clone());
        if result.claude_code_version.is_some() {
            self.version.clone_from(&result.claude_code_version);
        }
        if !result.output_style.is_empty() {
            self.output_style = Some(result.output_style.clone());
        }
        self.output_styles.clone_from(&result.available_output_styles);
    }

    /// Whether the CLI announced the protocol capability `name`.
    pub(crate) fn supports(&self, name: &str) -> bool {
        self.protocol.iter().any(|c| c == name)
    }

    /// Whether the model can call `tool`.
    #[cfg(test)]
    pub(crate) fn has_tool(&self, tool: &str) -> bool {
        self.tools.iter().any(|t| t == tool)
    }

    /// Whether the interrupt answer lists the messages that survive it.
    pub fn interrupt_receipts(&self) -> bool {
        self.supports("interrupt_receipt_v1")
    }

    /// Whether subagents can be started, as far as the tools tell.
    #[cfg(test)]
    pub(crate) fn subagents(&self) -> bool {
        self.has_tool("Agent") || self.has_tool("Task")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_read_from_init_and_initialize() {
        let init: Init = serde_json::from_value(serde_json::json!({
            "type": "system", "subtype": "init", "session_id": "s", "cwd": "/w", "model": "m",
            "permissionMode": "default", "tools": ["Agent", "Bash"], "claude_code_version": "2.1.319",
            "capabilities": ["interrupt_receipt_v1", "something_new_v9"],
            "mcp_servers": [{"name": "context7", "status": "pending", "source": "user"}]
        }))
        .unwrap();
        let mut caps = Capabilities::default();
        caps.learn_init(&init);
        assert!(caps.interrupt_receipts());
        assert!(caps.supports("something_new_v9"));
        assert!(caps.subagents());
        assert_eq!(caps.mcp_servers, [("context7".to_string(), "pending".to_string())]);
        assert_eq!(caps.version.as_deref(), Some("2.1.319"));
    }

    #[test]
    fn experimental_features_are_marked_as_such() {
        assert_eq!(Feature::Usage.tier(), Tier::Experimental);
        assert_eq!(Feature::RewindFiles.tier(), Tier::Experimental);
        assert_eq!(Feature::SessionList.tier(), Tier::Experimental);
        assert_eq!(Feature::BackgroundTasks.tier(), Tier::Core);
        assert_eq!(Feature::Permissions.tier(), Tier::Core);
    }
}
