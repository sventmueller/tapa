//! The requests behind the overlays of the agents' configuration (E146): internal requests of the CLI that the VS Code
//! extension sends the same way (`docs/claude-mapping.md`), and their answers, read leniently since the CLI marks
//! them internal and may add or drop fields.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{Answer, ControlError};

/// A request of an overlay.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DialogRequest {
    /// The MCP servers and their state.
    McpStatus,
    /// Turns the MCP server on or off for the session.
    McpToggle {
        /// The server, as `mcp_status` names it.
        server: String,
        /// On or off.
        enabled: bool,
    },
    /// Connects the MCP server again.
    McpReconnect(String),
    /// Starts the sign-in of the MCP server; the answer carries the address to open.
    McpAuthenticate(String),
    /// Forgets the sign-in of the MCP server.
    McpClearAuth(String),
    /// Hands the CLI the address the browser landed on after a sign-in, when it could not catch it itself.
    McpCallbackUrl {
        /// The server.
        server: String,
        /// The address with the sign-in's answer.
        url: String,
    },
    /// The settings as they apply, and each file's own.
    GetSettings,
    /// Writes the output style into the project's local settings, where `/config` writes it.
    SetOutputStyle(String),
    /// Reads the output styles again.
    ReloadOutputStyles,
    /// Reads the plugins again.
    ReloadPlugins,
    /// Reads the skills again.
    ReloadSkills,
    /// The permission rules in effect, with where each comes from.
    PermissionRules,
    /// The hooks, grouped as `/hooks` lists them.
    HooksListing,
    /// The rows of `/memory`.
    MemoryDialog,
    /// The rows of `/skills`.
    SkillsDialog,
    /// The rows of `/status`.
    Status,
    /// The conversation as `/export` writes it.
    ExportConversation,
    /// A question beside the conversation, which does not join it (`/btw`).
    SideQuestion {
        /// The question.
        question: String,
        /// The earlier questions and answers beside this conversation.
        history: Vec<SideExchange>,
    },
    /// A report to Anthropic: the description with the session's transcript and the CLI's redacted error log.
    SubmitFeedback {
        /// `bug` or `idea`.
        kind: String,
        /// Its title.
        title: String,
        /// What the user wrote.
        description: String,
    },
}

/// An earlier question beside the conversation and its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideExchange {
    /// The question.
    pub question: String,
    /// The answer.
    pub answer: String,
}

impl DialogRequest {
    /// The request's `subtype`.
    pub(crate) fn subtype(&self) -> &'static str {
        match self {
            DialogRequest::McpStatus => "mcp_status",
            DialogRequest::McpToggle { .. } => "mcp_toggle",
            DialogRequest::McpReconnect(_) => "mcp_reconnect",
            DialogRequest::McpAuthenticate(_) => "mcp_authenticate",
            DialogRequest::McpClearAuth(_) => "mcp_clear_auth",
            DialogRequest::McpCallbackUrl { .. } => "mcp_oauth_callback_url",
            DialogRequest::GetSettings => "get_settings",
            DialogRequest::SetOutputStyle(_) => "update_settings",
            DialogRequest::ReloadOutputStyles => "reload_output_styles",
            DialogRequest::ReloadPlugins => "reload_plugins",
            DialogRequest::ReloadSkills => "reload_skills",
            DialogRequest::PermissionRules => "list_permission_rules",
            DialogRequest::HooksListing => "get_hooks_listing",
            DialogRequest::MemoryDialog => "get_memory_dialog",
            DialogRequest::SkillsDialog => "get_skills_dialog",
            DialogRequest::Status => "get_status",
            DialogRequest::ExportConversation => "export_conversation",
            DialogRequest::SideQuestion { .. } => "side_question",
            DialogRequest::SubmitFeedback { .. } => "submit_feedback",
        }
    }

    /// How long the app waits for the answer: a question beside the conversation asks the model, a reconnect or
    /// sign-in waits for a server, the rest reads what the CLI holds.
    pub(crate) fn timeout(&self) -> Duration {
        match self {
            DialogRequest::SideQuestion { .. } => Duration::from_secs(300),
            DialogRequest::McpReconnect(_)
            | DialogRequest::McpAuthenticate(_)
            | DialogRequest::ReloadPlugins
            | DialogRequest::SubmitFeedback { .. } => Duration::from_secs(60),
            _ => Duration::from_secs(30),
        }
    }

    /// The request's fields, without its `subtype`.
    pub(crate) fn body(&self) -> Value {
        match self {
            DialogRequest::McpToggle { server, enabled } => json!({ "serverName": server, "enabled": enabled }),
            DialogRequest::McpReconnect(server)
            | DialogRequest::McpAuthenticate(server)
            | DialogRequest::McpClearAuth(server) => json!({ "serverName": server }),
            DialogRequest::McpCallbackUrl { server, url } => json!({ "serverName": server, "callbackUrl": url }),
            DialogRequest::SetOutputStyle(style) => {
                json!({ "source": "localSettings", "settings": { "outputStyle": style } })
            }
            DialogRequest::SideQuestion { question, history } if history.is_empty() => json!({ "question": question }),
            DialogRequest::SideQuestion { question, history } => {
                let history: Vec<Value> = history
                    .iter()
                    .map(|exchange| json!({ "question": exchange.question, "response": exchange.answer }))
                    .collect();
                json!({ "question": question, "history": history })
            }
            DialogRequest::SubmitFeedback {
                kind,
                title,
                description,
            } => json!({ "type": kind, "title": title, "description": description }),
            _ => json!({}),
        }
    }
}

/// An MCP server as `mcp_status` reports it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpServerState {
    /// Its name, as the requests take it.
    pub name: String,
    /// `connected`, `pending`, `failed`, `needs-auth`, `disabled`, or a state this build does not know.
    pub status: String,
    /// Why it failed.
    pub error: Option<String>,
    /// `stdio`, `http`, `sse`, `ws`, `sdk`, …
    pub transport: Option<String>,
    /// `local`, `project`, `user`, `plugin`, …
    pub scope: Option<String>,
    /// Where its definition comes from.
    pub source: Option<String>,
    /// Its tools, while it is connected.
    pub tools: Vec<Value>,
    /// Its definition.
    pub config: Value,
}

/// What `mcp_status` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpStatus {
    /// The servers.
    pub mcp_servers: Vec<McpServerState>,
}

/// What `mcp_authenticate` answers for an HTTP or SSE server: the address to open in the browser, when the sign-in
/// needs one.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct McpSignIn {
    /// The address.
    pub auth_url: Option<String>,
    /// Whether the user has to act in the browser.
    pub requires_user_action: bool,
    /// Why the sign-in could not start.
    pub error: Option<String>,
}

/// One settings file's own settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SettingsSource {
    /// `userSettings`, `projectSettings`, `localSettings`, `flagSettings` or `policySettings`.
    pub source: String,
    /// What the file sets.
    pub settings: Map<String, Value>,
}

/// What `get_settings` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SettingsReport {
    /// The settings as they apply.
    pub effective: Map<String, Value>,
    /// Each file's own, lowest priority first.
    pub sources: Vec<SettingsSource>,
    /// What the session runs with now, such as its model and effort.
    pub applied: Map<String, Value>,
}

/// How the CLI words a permission rule for people.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RuleWording {
    /// The text before the emphasis.
    pub prefix: String,
    /// The part to emphasise.
    pub emphasis: String,
}

/// A permission rule in effect.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PermissionRuleRow {
    /// `allow`, `ask` or `deny`.
    pub behavior: String,
    /// Where it lives, such as `localSettings`, `session` or `cliArg`.
    pub source: String,
    /// The rule, such as `Bash(git push *)`.
    pub rule: String,
    /// The CLI's wording of it.
    pub description: Option<RuleWording>,
    /// `persistent` for a rule in a file the user may change; anything else is read only.
    pub editability: String,
}

/// A folder the session may work in besides its own.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct WorkspaceFolder {
    /// The folder.
    pub path: String,
    /// Where the grant comes from.
    pub source: String,
}

/// The permission rules and folders in effect.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PermissionState {
    /// The rules.
    pub rules: Vec<PermissionRuleRow>,
    /// The extra folders.
    pub workspace_directories: Vec<WorkspaceFolder>,
    /// The folder the session started in.
    pub original_cwd: String,
    /// Whether only managed rules apply.
    pub managed_only: bool,
}

/// What `list_permission_rules` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct PermissionRules {
    /// The rules and folders.
    pub state: PermissionState,
}

/// A hook event the CLI knows.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HookEvent {
    /// Its name, such as `PreToolUse`.
    pub name: String,
    /// What it is, in a line.
    pub summary: String,
    /// Whether its hooks take a matcher.
    pub supports_matcher: bool,
}

/// What a hook in a settings file is, so an edit can name it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookTarget {
    /// Its matcher.
    pub matcher: String,
    /// Its definition, as the file holds it.
    pub config: Value,
}

/// A hook as `/hooks` lists it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HookRow {
    /// The event it runs on.
    pub event: String,
    /// Its matcher; empty for none.
    pub matcher: String,
    /// Where it lives, such as `userSettings` or `pluginHook`.
    pub source: String,
    /// Where it lives, for people.
    pub source_label: String,
    /// `command`, `prompt`, `agent`, `http`, `mcp_tool`, …
    #[serde(rename = "type")]
    pub kind: String,
    /// What the list row shows.
    pub display_text: String,
    /// What runs.
    pub command_text: String,
    /// Its timeout in seconds.
    pub timeout: Option<u64>,
    /// Present for a hook the user may change.
    pub editable: Option<HookTarget>,
}

/// What `get_hooks_listing` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HooksListing {
    /// The hooks.
    pub hooks: Vec<HookRow>,
    /// Every event, in the order `/hooks` lists them.
    pub event_catalog: Vec<HookEvent>,
}

/// An instruction file `/memory` lists.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryFile {
    /// `user`, `project`, `local`, …
    pub kind: String,
    /// The file.
    pub path: String,
    /// Its name for people.
    pub label: String,
    /// Where it lives, for people.
    pub description: String,
    /// Whether it exists yet.
    pub exists: bool,
}

/// A folder `/memory` offers to open.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryFolder {
    /// `auto`, …
    pub kind: String,
    /// The folder.
    pub path: String,
    /// The row's label.
    pub label: String,
}

/// A memory switch.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryToggle {
    /// Whether it is on.
    pub enabled: bool,
    /// Whether the user may switch it here.
    pub toggleable: bool,
}

/// What `get_memory_dialog` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MemoryDialog {
    /// The instruction files.
    pub files: Vec<MemoryFile>,
    /// The folders.
    pub folders: Vec<MemoryFolder>,
    /// Automatic memory.
    pub auto_memory: MemoryToggle,
}

/// A skill as `/skills` lists it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SkillRow {
    /// Its name, the key of its setting.
    pub name: String,
    /// The name shown.
    pub display_name: String,
    /// What it does.
    pub description: String,
    /// `user`, `project`, `plugin`, `built-in`, …
    pub source: String,
    /// About how many tokens its listing costs.
    pub tokens: u64,
    /// `on`, `name-only`, `user-invocable-only` or `off`.
    pub state: String,
    /// What the CLI needs back to change it.
    pub handles: Value,
}

/// What `get_skills_dialog` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SkillsDialog {
    /// The skills.
    pub skills: Vec<SkillRow>,
}

/// A row of `/status`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StatusRow {
    /// Its name; none for a row that is a sentence on its own.
    pub label: Option<String>,
    /// Its value as text.
    pub value: String,
}

/// A section of `/status`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StatusSection {
    /// Its title.
    pub title: String,
    /// Its rows.
    pub rows: Vec<StatusRow>,
}

/// What `get_status` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StatusReport {
    /// The sections, in their order.
    pub sections: Vec<StatusSection>,
}

/// What `export_conversation` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ExportedConversation {
    /// The conversation as plain text.
    pub text: String,
    /// The file name `/export` would offer.
    pub default_filename: String,
}

/// The answer another model gave after the session's model refused a question beside the conversation.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RefusalFallback {
    /// The model that refused.
    pub original_model: String,
    /// The model that answered.
    pub fallback_model: String,
    /// The notice to show with the answer.
    pub content: String,
}

/// What `side_question` answers.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SideAnswer {
    /// The answer; none when the question was cancelled.
    pub response: Option<String>,
    /// Whether the CLI made it up itself instead of asking the model, such as for an error.
    pub synthetic: bool,
    /// Present when another model answered after a refusal.
    pub refusal_fallback: Option<RefusalFallback>,
}

/// What `submit_feedback` answers: an id when the report went out or was saved, else why not.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FeedbackResult {
    /// The report's id, when it was sent or saved.
    pub feedback_id: Option<String>,
    /// Where it was saved instead of sent, as a redacted zip on this Mac.
    pub bundle_path: Option<String>,
    /// Why it cannot be sent in this session.
    pub unavailable_reason: Option<String>,
    /// Why sending it failed.
    pub failure_reason: Option<String>,
}

macro_rules! answer_leniently {
    ($($t:ty),*) => {$(
        impl Answer for $t {
            fn read(payload: Value) -> Result<$t, ControlError> {
                <$t>::deserialize(&payload).map_err(|e| ControlError::Unexpected(e.to_string()))
            }
        }
    )*};
}

answer_leniently!(
    McpStatus,
    McpSignIn,
    SettingsReport,
    PermissionRules,
    HooksListing,
    MemoryDialog,
    SkillsDialog,
    StatusReport,
    ExportedConversation,
    SideAnswer,
    FeedbackResult
);

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture<T: Answer>(name: &str) -> T {
        let path = format!("{}/tests/dialog_fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(path).unwrap();
        T::read(serde_json::from_str(&text).unwrap()).unwrap()
    }

    #[test]
    fn the_recorded_answers_read() {
        let mcp: McpStatus = fixture("mcp_status");
        assert_eq!(mcp.mcp_servers.len(), 2);
        assert_eq!(mcp.mcp_servers[0].status, "failed");
        assert!(
            mcp.mcp_servers[0]
                .error
                .as_deref()
                .is_some_and(|e| e.contains("ENOENT"))
        );
        assert_eq!(mcp.mcp_servers[1].transport.as_deref(), Some("http"));
        let settings: SettingsReport = fixture("get_settings");
        assert_eq!(settings.effective["outputStyle"], "Explanatory");
        assert_eq!(settings.sources[0].source, "userSettings");
        let rules: PermissionRules = fixture("list_permission_rules");
        assert_eq!(rules.state.rules.len(), 4);
        assert_eq!(rules.state.rules[2].behavior, "ask");
        assert_eq!(rules.state.rules[0].editability, "persistent");
        let hooks: HooksListing = fixture("get_hooks_listing");
        assert_eq!(hooks.hooks[0].command_text, "echo check");
        assert_eq!(
            hooks.hooks[0].editable.as_ref().map(|e| e.matcher.as_str()),
            Some("Bash")
        );
        assert!(
            hooks
                .event_catalog
                .iter()
                .any(|e| e.name == "PreToolUse" && e.supports_matcher)
        );
        let memory: MemoryDialog = fixture("get_memory_dialog");
        assert!(memory.files[0].exists && !memory.files[1].exists);
        assert!(memory.auto_memory.enabled && memory.auto_memory.toggleable);
        let skills: SkillsDialog = fixture("get_skills_dialog");
        assert_eq!(
            (skills.skills[0].name.as_str(), skills.skills[0].state.as_str()),
            ("demo", "on")
        );
        let status: StatusReport = fixture("get_status");
        assert_eq!(status.sections[0].rows[0].label.as_deref(), Some("Version"));
        let export: ExportedConversation = fixture("export_conversation");
        assert!(export.default_filename.ends_with(".txt"));
    }

    #[test]
    fn unknown_fields_and_missing_ones_still_read() {
        let mcp = McpStatus::read(json!({ "mcpServers": [{ "name": "x", "future": 1 }], "extra": true })).unwrap();
        assert_eq!(mcp.mcp_servers[0].name, "x");
        assert_eq!(mcp.mcp_servers[0].status, "");
        assert_eq!(StatusReport::read(json!({})).unwrap(), StatusReport::default());
    }

    #[test]
    fn requests_carry_the_fields_the_cli_reads() {
        let toggle = DialogRequest::McpToggle {
            server: "docs".into(),
            enabled: false,
        };
        assert_eq!(toggle.subtype(), "mcp_toggle");
        assert_eq!(toggle.body(), json!({ "serverName": "docs", "enabled": false }));
        let style = DialogRequest::SetOutputStyle("Explanatory".into());
        assert_eq!(
            (style.subtype(), style.body()),
            (
                "update_settings",
                json!({ "source": "localSettings", "settings": { "outputStyle": "Explanatory" } })
            )
        );
        let aside = DialogRequest::SideQuestion {
            question: "why?".into(),
            history: vec![SideExchange {
                question: "what?".into(),
                answer: "this".into(),
            }],
        };
        assert_eq!(
            aside.body(),
            json!({ "question": "why?", "history": [{ "question": "what?", "response": "this" }] })
        );
        assert!(aside.timeout() > DialogRequest::McpStatus.timeout());
        let first = DialogRequest::SideQuestion {
            question: "why?".into(),
            history: Vec::new(),
        };
        assert_eq!(
            first.body(),
            json!({ "question": "why?" }),
            "an empty history is left out, as the CLI expects"
        );
        let answer = SideAnswer::read(json!({ "response": null, "synthetic": false })).unwrap();
        assert_eq!(answer.response, None);
    }

    #[test]
    fn a_report_names_its_kind_and_its_answer_says_where_it_went() {
        let report = DialogRequest::SubmitFeedback {
            kind: "bug".into(),
            title: "Crash".into(),
            description: "It fell over.".into(),
        };
        assert_eq!(report.subtype(), "submit_feedback");
        assert_eq!(
            report.body(),
            json!({ "type": "bug", "title": "Crash", "description": "It fell over." })
        );
        let saved = FeedbackResult::read(json!({ "feedback_id": "b1", "bundle_path": "/tmp/r.zip" })).unwrap();
        assert_eq!(saved.bundle_path.as_deref(), Some("/tmp/r.zip"));
        let refused = FeedbackResult::read(json!({ "feedback_id": null, "unavailable_reason": "off" })).unwrap();
        assert_eq!(
            (refused.feedback_id, refused.unavailable_reason.as_deref()),
            (None, Some("off"))
        );
    }
}
