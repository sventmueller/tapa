//! What the management calls of a session take and answer: stored threads, configuration, MCP servers, skills, the
//! account and feedback.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::capabilities::Account;
use crate::session::SessionError;

/// Which stored threads `Session::threads` lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadQuery {
    /// Only threads of this folder.
    pub cwd: Option<PathBuf>,
    /// Only threads whose title contains this.
    pub search: Option<String>,
    /// At most this many.
    pub limit: Option<u32>,
    /// Where the previous page ended.
    pub cursor: Option<String>,
    /// Archived threads instead of the others.
    pub archived: bool,
}

impl ThreadQuery {
    pub(crate) fn params(&self) -> Value {
        let mut params = json!({"sortKey": "updated_at"});
        if let Some(cwd) = &self.cwd {
            params["cwd"] = json!(cwd.to_string_lossy());
        }
        if let Some(search) = &self.search {
            params["searchTerm"] = json!(search);
        }
        if let Some(limit) = self.limit {
            params["limit"] = json!(limit);
        }
        if let Some(cursor) = &self.cursor {
            params["cursor"] = json!(cursor);
        }
        if self.archived {
            params["archived"] = json!(true);
        }
        params
    }
}

/// A stored thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    /// Codex's id, which resuming names.
    pub id: String,
    /// Its title, if it has one.
    #[serde(default)]
    pub name: Option<String>,
    /// Usually its first user message.
    #[serde(default)]
    pub preview: String,
    /// The folder it worked in.
    #[serde(default)]
    pub cwd: PathBuf,
    /// When it was created, in seconds since 1970.
    #[serde(default)]
    pub created_at: i64,
    /// When it changed last, in seconds since 1970.
    #[serde(default)]
    pub updated_at: i64,
    /// Its model.
    #[serde(default)]
    pub model: Option<String>,
    /// The thread it was forked from.
    #[serde(default)]
    pub forked_from_id: Option<String>,
}

/// One page of stored threads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    /// The threads, the latest change first.
    #[serde(rename = "data")]
    pub threads: Vec<ThreadSummary>,
    /// Where the next page starts; `None` on the last.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// The answer to `thread/fork`: the new thread.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Forked {
    pub(crate) thread: ThreadSummary,
}

/// How a value is written into `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStrategy {
    /// Replaces what was there.
    Replace,
    /// Merges a table into what was there.
    Upsert,
}

impl MergeStrategy {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            MergeStrategy::Replace => "replace",
            MergeStrategy::Upsert => "upsert",
        }
    }
}

/// One value to write into `config.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEdit {
    /// Where, as a dotted path such as `mcp_servers.docs`.
    pub key_path: String,
    /// The value.
    pub value: Value,
    /// How it is written.
    pub merge: MergeStrategy,
}

impl ConfigEdit {
    pub(crate) fn json(&self) -> Value {
        json!({"keyPath": self.key_path, "value": self.value, "mergeStrategy": self.merge.wire()})
    }
}

/// The answer to a configuration write.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigWritten {
    /// The file written.
    pub file_path: PathBuf,
    /// Its version now, which the next write may name.
    pub version: String,
    /// `ok`, or `okOverridden` when a layer above overrides the value.
    pub status: String,
}

/// An MCP server as `mcpServerStatus/list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// Its name in the configuration.
    pub name: String,
    /// `notLoggedIn`, `oAuth`, `bearerToken`, `unsupported` or `unknown`.
    pub auth: String,
    /// The names of its tools.
    pub tools: Vec<String>,
    /// Why its tools could not be listed.
    pub error: Option<String>,
}

pub(crate) fn mcp_servers(result: Value) -> Result<Vec<McpServer>, SessionError> {
    let list = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| SessionError::Malformed("mcpServerStatus/list has no data".into()))?;
    Ok(list
        .iter()
        .filter_map(|server| {
            let mut tools: Vec<String> = server
                .get("tools")
                .and_then(Value::as_object)
                .map(|t| t.keys().cloned().collect())
                .unwrap_or_default();
            tools.sort();
            Some(McpServer {
                name: server.get("name")?.as_str()?.to_string(),
                auth: server
                    .get("authStatus")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                tools,
                error: server.get("toolsError").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect())
}

/// The page that signs in to an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpLogin {
    /// The page to open.
    pub authorization_url: String,
    /// The id the completion names.
    #[serde(default)]
    pub login_id: Option<String>,
}

/// A skill.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// Its name.
    pub name: String,
    /// What it does.
    #[serde(default)]
    pub description: String,
    /// Its `SKILL.md`.
    pub path: PathBuf,
    /// Whether Codex uses it.
    pub enabled: bool,
    /// Where it comes from, such as `user` or `repo`.
    #[serde(default)]
    pub scope: String,
}

/// The skills of one folder.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Skills {
    /// The folder.
    pub cwd: PathBuf,
    /// Its skills.
    pub skills: Vec<Skill>,
}

pub(crate) fn skills(result: Value) -> Result<Vec<Skills>, SessionError> {
    #[derive(Deserialize)]
    struct List {
        data: Vec<Skills>,
    }
    crate::session::typed::<List>(result).map(|l| l.data)
}

pub(crate) fn skill_enabled(result: Value) -> Result<bool, SessionError> {
    result
        .get("effectiveEnabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| SessionError::Malformed("skills/config/write has no effectiveEnabled".into()))
}

/// The account and whether one is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountState {
    /// How Codex is signed in; `None` when it is not.
    pub account: Option<Account>,
    /// Whether the configured provider needs an OpenAI sign-in.
    pub requires_openai_auth: bool,
}

pub(crate) fn account(result: Value) -> Result<AccountState, SessionError> {
    Ok(AccountState {
        account: result.get("account").and_then(Account::from_json),
        requires_openai_auth: result
            .get("requiresOpenaiAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// How to sign in.
#[derive(Clone, PartialEq, Eq)]
pub enum Login {
    /// With ChatGPT in the browser, which Codex opens a local callback for.
    ChatGpt,
    /// With ChatGPT on another device, by a code.
    DeviceCode,
    /// With an OpenAI API key.
    ApiKey(String),
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Login::ChatGpt => f.write_str("ChatGpt"),
            Login::DeviceCode => f.write_str("DeviceCode"),
            Login::ApiKey(_) => f.write_str("ApiKey(..)"),
        }
    }
}

impl Login {
    pub(crate) fn params(&self) -> Value {
        match self {
            Login::ChatGpt => json!({"type": "chatgpt"}),
            Login::DeviceCode => json!({"type": "chatgptDeviceCode"}),
            Login::ApiKey(key) => json!({"type": "apiKey", "apiKey": key}),
        }
    }
}

/// A sign-in that has started; `Event::Login` says when it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginStarted {
    /// The key was taken; nothing is left to do.
    ApiKey,
    /// The user signs in on this page.
    Browser {
        /// The page.
        url: String,
        /// The id the completion names.
        login_id: String,
    },
    /// The user enters `user_code` on `verification_url`.
    DeviceCode {
        /// The id the completion names.
        login_id: String,
        /// The code.
        user_code: String,
        /// The page.
        verification_url: String,
    },
}

pub(crate) fn login_started(result: Value) -> Result<LoginStarted, SessionError> {
    let field = |name: &str| {
        result
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| SessionError::Malformed(format!("account/login/start has no {name}")))
    };
    match result.get("type").and_then(Value::as_str) {
        Some("apiKey") => Ok(LoginStarted::ApiKey),
        Some("chatgpt") => Ok(LoginStarted::Browser {
            url: field("authUrl")?,
            login_id: field("loginId")?,
        }),
        Some("chatgptDeviceCode") => Ok(LoginStarted::DeviceCode {
            login_id: field("loginId")?,
            user_code: field("userCode")?,
            verification_url: field("verificationUrl")?,
        }),
        other => Err(SessionError::Malformed(format!(
            "account/login/start answered {other:?}"
        ))),
    }
}

/// Feedback for OpenAI about the thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    /// What sort of feedback, such as `bug` or `good_result`.
    pub classification: String,
    /// What the user wrote.
    pub reason: Option<String>,
    /// Whether Codex's logs of the thread go with it.
    pub include_logs: bool,
}

/// The answer to an upload of feedback.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSent {
    /// The thread it was about.
    pub thread_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_of_the_management_calls_read_into_their_types() {
        let servers = mcp_servers(json!({"data": [{"name": "docs", "authStatus": "notLoggedIn",
            "tools": {"search": {}, "fetch": {}}, "resources": [], "resourceTemplates": []}]}))
        .unwrap();
        assert_eq!(servers[0].tools, ["fetch", "search"]);
        assert_eq!(servers[0].auth, "notLoggedIn");
        let started = login_started(
            json!({"type": "chatgptDeviceCode", "loginId": "l1", "userCode": "AB-12",
            "verificationUrl": "https://example.test/device"}),
        )
        .unwrap();
        assert!(matches!(started, LoginStarted::DeviceCode { user_code, .. } if user_code == "AB-12"));
        assert!(login_started(json!({"type": "chatgpt"})).is_err());
        let key = Login::ApiKey("sk-secret".into());
        assert_eq!(key.params()["apiKey"], "sk-secret");
        assert_eq!(format!("{key:?}"), "ApiKey(..)");
        let page: ThreadPage = crate::session::typed(json!({"data": [{"id": "thr_1", "preview": "Fix it",
            "cwd": "/w", "createdAt": 1, "updatedAt": 2, "modelProvider": "openai"}], "nextCursor": null}))
        .unwrap();
        assert_eq!(page.threads[0].id, "thr_1");
        let query = ThreadQuery {
            cwd: Some("/w".into()),
            limit: Some(20),
            ..ThreadQuery::default()
        };
        assert_eq!(
            query.params(),
            json!({"sortKey": "updated_at", "cwd": "/w", "limit": 20})
        );
    }
}
