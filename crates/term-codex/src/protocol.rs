//! The parts of the app-server protocol v2 this crate reads, as of Codex 0.167 (`docs/codex.md`). Items are read
//! leniently: unknown item types and fields are skipped, never fatal.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// The method names this crate sends or handles, for the schema test to check against the server's.
pub mod method {
    /// The handshake request.
    pub const INITIALIZE: &str = "initialize";
    /// The notification that ends the handshake.
    pub const INITIALIZED: &str = "initialized";
    /// Starts a thread.
    pub const THREAD_START: &str = "thread/start";
    /// Loads a stored thread.
    pub const THREAD_RESUME: &str = "thread/resume";
    /// Copies a thread into a new one.
    pub const THREAD_FORK: &str = "thread/fork";
    /// Replaces the stored history with the turns before one; files stay as they are.
    pub const THREAD_REVERT: &str = "thread/revert";
    /// Lists stored threads.
    pub const THREAD_LIST: &str = "thread/list";
    /// Starts a turn.
    pub const TURN_START: &str = "turn/start";
    /// Adds input to the running turn.
    pub const TURN_STEER: &str = "turn/steer";
    /// Stops the running turn.
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    /// Lists the PTY processes still running beside a thread (experimental).
    pub const BACKGROUND_LIST: &str = "thread/backgroundTerminals/list";
    /// Stops one of a thread's background PTY processes (experimental).
    pub const BACKGROUND_TERMINATE: &str = "thread/backgroundTerminals/terminate";
    /// Lists the models.
    pub const MODEL_LIST: &str = "model/list";
    /// Reads the effective configuration.
    pub const CONFIG_READ: &str = "config/read";
    /// Writes one value of `config.toml`.
    pub const CONFIG_VALUE_WRITE: &str = "config/value/write";
    /// Writes several values of `config.toml`.
    pub const CONFIG_BATCH_WRITE: &str = "config/batchWrite";
    /// Lists the MCP servers and their state.
    pub const MCP_STATUS_LIST: &str = "mcpServerStatus/list";
    /// Starts the OAuth sign-in of an MCP server.
    pub const MCP_OAUTH_LOGIN: &str = "mcpServer/oauth/login";
    /// Reloads the MCP servers from the configuration.
    pub const MCP_RELOAD: &str = "config/mcpServer/reload";
    /// Lists the skills.
    pub const SKILLS_LIST: &str = "skills/list";
    /// Turns a skill on or off.
    pub const SKILLS_CONFIG_WRITE: &str = "skills/config/write";
    /// Reads the account.
    pub const ACCOUNT_READ: &str = "account/read";
    /// Signs out.
    pub const ACCOUNT_LOGOUT: &str = "account/logout";
    /// Starts a sign-in.
    pub const ACCOUNT_LOGIN_START: &str = "account/login/start";
    /// Stops a sign-in that has not completed.
    pub const ACCOUNT_LOGIN_CANCEL: &str = "account/login/cancel";
    /// Reads the plan's usage limits.
    pub const ACCOUNT_RATE_LIMITS_READ: &str = "account/rateLimits/read";
    /// Sends feedback with the thread's logs to OpenAI.
    pub const FEEDBACK_UPLOAD: &str = "feedback/upload";
    /// Lists the plugins.
    pub const PLUGIN_LIST: &str = "plugin/list";

    /// The server asks before it runs a command.
    pub const COMMAND_APPROVAL: &str = "item/commandExecution/requestApproval";
    /// The server asks before it changes files.
    pub const FILE_CHANGE_APPROVAL: &str = "item/fileChange/requestApproval";
    /// The server asks for more permissions for a command.
    pub const PERMISSIONS_APPROVAL: &str = "item/permissions/requestApproval";
    /// The server asks the user questions.
    pub const USER_INPUT: &str = "item/tool/requestUserInput";
    /// An MCP server asks the user for input or to open a page.
    pub const ELICITATION: &str = "mcpServer/elicitation/request";

    /// The notifications the mapper reads.
    pub const NOTIFICATIONS: [&str; 28] = [
        "error",
        "thread/started",
        "thread/status/changed",
        "thread/closed",
        "thread/name/updated",
        "thread/tokenUsage/updated",
        "turn/started",
        "turn/completed",
        "turn/diff/updated",
        "turn/plan/updated",
        "item/started",
        "item/completed",
        "item/agentMessage/delta",
        "item/plan/delta",
        "item/commandExecution/outputDelta",
        "item/fileChange/patchUpdated",
        "item/reasoning/summaryTextDelta",
        "item/reasoning/summaryPartAdded",
        "item/reasoning/textDelta",
        "serverRequest/resolved",
        "account/updated",
        "account/rateLimits/updated",
        "account/login/completed",
        "mcpServer/oauthLogin/completed",
        "model/rerouted",
        "warning",
        "configWarning",
        "deprecationNotice",
    ];
}

/// One change to one file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct FileChange {
    pub(crate) path: String,
    /// `{"type": "add" | "delete" | "update", "move_path"?}`.
    #[serde(default)]
    pub(crate) kind: Value,
    /// A unified diff for an update; the whole content for an added or deleted file.
    #[serde(default)]
    pub(crate) diff: String,
}

impl FileChange {
    pub(crate) fn kind(&self) -> &str {
        self.kind.get("type").and_then(Value::as_str).unwrap_or("update")
    }
}

/// One of the parsed actions of a command, for a friendlier card.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum CommandAction {
    Read {
        #[serde(default)]
        path: String,
    },
    ListFiles {
        path: Option<String>,
    },
    Search {
        query: Option<String>,
        path: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

/// A thread item; the payload is flat on the wire, tagged by `type`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum Item {
    #[serde(rename_all = "camelCase")]
    UserMessage {
        id: String,
        #[serde(default)]
        content: Vec<Value>,
        client_id: Option<String>,
    },
    AgentMessage {
        id: String,
        #[serde(default)]
        text: String,
    },
    Plan {
        id: String,
        #[serde(default)]
        text: String,
    },
    Reasoning {
        id: String,
        #[serde(default)]
        summary: Vec<String>,
        #[serde(default)]
        content: Vec<String>,
    },
    #[serde(rename_all = "camelCase")]
    CommandExecution {
        id: String,
        command: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        command_actions: Vec<CommandAction>,
        #[serde(default)]
        status: String,
        aggregated_output: Option<String>,
        exit_code: Option<i64>,
        duration_ms: Option<u64>,
        process_id: Option<String>,
    },
    FileChange {
        id: String,
        #[serde(default)]
        changes: Vec<FileChange>,
        #[serde(default)]
        status: String,
    },
    #[serde(rename_all = "camelCase")]
    McpToolCall {
        id: String,
        server: String,
        tool: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        status: String,
        result: Option<Value>,
        error: Option<Value>,
        duration_ms: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    DynamicToolCall {
        id: String,
        tool: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default)]
        status: String,
        content_items: Option<Vec<Value>>,
        duration_ms: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    CollabAgentToolCall {
        id: String,
        tool: String,
        prompt: Option<String>,
        #[serde(default)]
        status: String,
        #[serde(default)]
        receiver_thread_ids: Vec<String>,
        #[serde(default)]
        agents_states: BTreeMap<String, AgentState>,
    },
    WebSearch {
        id: String,
        #[serde(default)]
        query: String,
        action: Option<Value>,
    },
    ImageView {
        id: String,
        path: String,
    },
    EnteredReviewMode {
        id: String,
        #[serde(default)]
        review: String,
    },
    ExitedReviewMode {
        id: String,
        #[serde(default)]
        review: String,
    },
    ContextCompaction {
        id: String,
    },
    #[serde(other)]
    Other,
}

/// The state reported by a collaboration tool for one of its receivers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct AgentState {
    pub(crate) status: String,
    pub(crate) message: Option<String>,
}

impl Item {
    pub(crate) fn read(value: &Value) -> Option<Item> {
        Item::deserialize(value).ok()
    }

    pub(crate) fn id(&self) -> Option<&str> {
        match self {
            Item::UserMessage { id, .. }
            | Item::AgentMessage { id, .. }
            | Item::Plan { id, .. }
            | Item::Reasoning { id, .. }
            | Item::CommandExecution { id, .. }
            | Item::FileChange { id, .. }
            | Item::McpToolCall { id, .. }
            | Item::DynamicToolCall { id, .. }
            | Item::CollabAgentToolCall { id, .. }
            | Item::WebSearch { id, .. }
            | Item::ImageView { id, .. }
            | Item::EnteredReviewMode { id, .. }
            | Item::ExitedReviewMode { id, .. }
            | Item::ContextCompaction { id } => Some(id),
            Item::Other => None,
        }
    }
}

/// The text of a string field of `value`.
pub(crate) fn text<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(Value::as_str)
}

/// The number of a field of `value`.
pub(crate) fn number(value: &Value, name: &str) -> Option<u64> {
    value.get(name).and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn items_read_by_type_and_unknown_ones_become_other() {
        let item = Item::read(
            &json!({"type": "commandExecution", "id": "c1", "command": "ls", "cwd": "/w",
            "commandActions": [{"type": "listFiles", "command": "ls", "path": null}, {"type": "future"}],
            "status": "inProgress", "startedAtMs": 1}),
        );
        let Some(Item::CommandExecution { command_actions, .. }) = item else {
            panic!("{item:?}")
        };
        assert_eq!(
            command_actions,
            [CommandAction::ListFiles { path: None }, CommandAction::Unknown]
        );
        assert_eq!(
            Item::read(&json!({"type": "sleep", "id": "s", "durationMs": 5})),
            Some(Item::Other)
        );
        assert_eq!(Item::read(&json!({"type": "agentMessage"})), None);
    }
}
