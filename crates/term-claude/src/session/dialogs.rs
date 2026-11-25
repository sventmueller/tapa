//! What the overlays of the agents' configuration ask the running CLI (E146); each answer arrives through the
//! returned [`Reply`].

use serde_json::Value;

use super::{Reply, Session};
use crate::control::{
    DialogRequest, ExportedConversation, FeedbackResult, HooksListing, McpSignIn, McpStatus, MemoryDialog,
    PermissionRules, Request, SettingsReport, SideAnswer, SideExchange, SkillsDialog, StatusReport,
};

impl Session {
    fn dialog<T: crate::control::Answer>(&self, request: DialogRequest) -> Reply<T> {
        self.request(Request::Dialog(request), None)
    }

    /// The MCP servers and their state.
    pub fn mcp_status(&self) -> Reply<McpStatus> {
        self.dialog(DialogRequest::McpStatus)
    }

    /// Turns the MCP server `server` on or off for the session.
    pub fn mcp_toggle(&self, server: &str, enabled: bool) -> Reply<()> {
        self.dialog(DialogRequest::McpToggle {
            server: server.to_string(),
            enabled,
        })
    }

    /// Connects the MCP server `server` again.
    pub fn mcp_reconnect(&self, server: &str) -> Reply<()> {
        self.dialog(DialogRequest::McpReconnect(server.to_string()))
    }

    /// Starts the sign-in of the MCP server `server`.
    pub fn mcp_sign_in(&self, server: &str) -> Reply<McpSignIn> {
        self.dialog(DialogRequest::McpAuthenticate(server.to_string()))
    }

    /// Forgets the sign-in of the MCP server `server`.
    pub fn mcp_sign_out(&self, server: &str) -> Reply<()> {
        self.dialog(DialogRequest::McpClearAuth(server.to_string()))
    }

    /// Hands over the address the browser landed on after signing in to the MCP server `server`.
    pub fn mcp_callback_url(&self, server: &str, url: &str) -> Reply<()> {
        self.dialog(DialogRequest::McpCallbackUrl {
            server: server.to_string(),
            url: url.to_string(),
        })
    }

    /// The settings as they apply, and each file's own.
    pub fn settings(&self) -> Reply<SettingsReport> {
        self.dialog(DialogRequest::GetSettings)
    }

    /// Sets the output style for this project, where `/config` sets it.
    pub fn set_output_style(&self, style: &str) -> Reply<()> {
        self.dialog(DialogRequest::SetOutputStyle(style.to_string()))
    }

    /// Reads the output styles again, after one changed.
    pub fn reload_output_styles(&self) -> Reply<Value> {
        self.dialog(DialogRequest::ReloadOutputStyles)
    }

    /// Reads the plugins again, after one was installed, removed or switched.
    pub fn reload_plugins(&self) -> Reply<Value> {
        self.dialog(DialogRequest::ReloadPlugins)
    }

    /// Reads the skills again, after one was switched.
    pub fn reload_skills(&self) -> Reply<Value> {
        self.dialog(DialogRequest::ReloadSkills)
    }

    /// The permission rules and extra folders in effect.
    pub fn permission_rules(&self) -> Reply<PermissionRules> {
        self.dialog(DialogRequest::PermissionRules)
    }

    /// The hooks, as `/hooks` lists them.
    pub fn hooks_listing(&self) -> Reply<HooksListing> {
        self.dialog(DialogRequest::HooksListing)
    }

    /// The rows of `/memory`.
    pub fn memory_dialog(&self) -> Reply<MemoryDialog> {
        self.dialog(DialogRequest::MemoryDialog)
    }

    /// The rows of `/skills`.
    pub fn skills_dialog(&self) -> Reply<SkillsDialog> {
        self.dialog(DialogRequest::SkillsDialog)
    }

    /// The rows of `/status`.
    pub fn status_report(&self) -> Reply<StatusReport> {
        self.dialog(DialogRequest::Status)
    }

    /// The conversation as `/export` writes it.
    pub fn export_conversation(&self) -> Reply<ExportedConversation> {
        self.dialog(DialogRequest::ExportConversation)
    }

    /// Asks `question` beside the conversation, after the earlier questions `history`; neither joins it (`/btw`).
    pub fn side_question(&self, question: &str, history: Vec<SideExchange>) -> Reply<SideAnswer> {
        self.dialog(DialogRequest::SideQuestion {
            question: question.to_string(),
            history,
        })
    }

    /// Sends a report to Anthropic with `description` and the session's transcript (`/bug`); `kind` is `bug` or
    /// `idea`.
    pub fn submit_feedback(&self, kind: &str, title: &str, description: &str) -> Reply<FeedbackResult> {
        self.dialog(DialogRequest::SubmitFeedback {
            kind: kind.to_string(),
            title: title.to_string(),
            description: description.to_string(),
        })
    }
}
