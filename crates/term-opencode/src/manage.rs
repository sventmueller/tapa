//! What the management calls of a session take and answer: the server, stored sessions, MCP servers, reverting,
//! and the credentials, without their secrets.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::capabilities::ModelRef;
use crate::http::encode;
use crate::session::SessionError;

fn text(value: &Value, name: &str) -> Option<String> {
    value.get(name).and_then(Value::as_str).map(str::to_string)
}

fn time(value: Option<&Value>, name: &str) -> Option<SystemTime> {
    let since = crate::mapping::duration_ms(value?.get(name)?).filter(|d| !d.is_zero())?;
    Some(UNIX_EPOCH + since)
}

fn malformed(what: &str) -> SessionError {
    SessionError::Malformed(format!("{what} has another shape than the schema"))
}

/// The server as `GET /api/info` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// Its release.
    pub version: String,
    /// Its process id.
    pub pid: u64,
    /// The URLs it listens on.
    pub urls: Vec<String>,
}

pub(crate) fn server_info(result: Value) -> Result<ServerInfo, SessionError> {
    Ok(ServerInfo {
        version: text(&result, "version").ok_or_else(|| malformed("server.info"))?,
        pid: result.get("pid").and_then(Value::as_u64).unwrap_or_default(),
        urls: result
            .get("urls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|u| u.as_str().map(str::to_string))
            .collect(),
    })
}

/// Where an MCP server stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStatus {
    /// Its tools are there.
    Connected,
    /// It is being connected.
    Pending,
    /// The configuration turns it off.
    Disabled,
    /// Connecting failed, for this reason.
    Failed(String),
    /// It wants a sign-in, for this reason.
    NeedsAuth(String),
}

/// An MCP server as `GET /api/mcp` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// Its name in the configuration.
    pub name: String,
    /// What the user reads, if it differs from the name.
    pub title: Option<String>,
    /// Where it stands.
    pub status: McpStatus,
}

pub(crate) fn mcp_servers(result: Value) -> Result<Vec<McpServer>, SessionError> {
    let list = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("mcp.list"))?;
    Ok(list
        .iter()
        .filter_map(|server| {
            let status = server.get("status")?;
            let why = text(status, "error").unwrap_or_default();
            Some(McpServer {
                name: text(server, "name")?,
                title: text(server, "title"),
                status: match status.get("status")?.as_str()? {
                    "connected" => McpStatus::Connected,
                    "pending" => McpStatus::Pending,
                    "disabled" => McpStatus::Disabled,
                    "needs_auth" => McpStatus::NeedsAuth(why),
                    _ => McpStatus::Failed(why),
                },
            })
        })
        .collect())
}

/// How OpenCode reaches an MCP server, as `PUT /api/experimental/mcp/{server}` takes it; `Debug` names the
/// variables and headers without their values, which often are tokens (R42).
#[derive(Clone, PartialEq, Eq)]
pub enum McpConfig {
    /// A program OpenCode starts.
    Local {
        /// The program and its arguments.
        command: Vec<String>,
        /// Its environment beyond OpenCode's.
        environment: Vec<(String, String)>,
        /// Where it runs.
        cwd: Option<PathBuf>,
        /// Whether it stays off.
        disabled: bool,
    },
    /// A server at a URL.
    Remote {
        /// Its URL.
        url: String,
        /// Headers every request carries.
        headers: Vec<(String, String)>,
        /// Whether it stays off.
        disabled: bool,
    },
}

fn pairs(pairs: &[(String, String)]) -> Value {
    Value::Object(pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<Map<_, _>>())
}

fn names(pairs: &[(String, String)]) -> Vec<&str> {
    pairs.iter().map(|(name, _)| name.as_str()).collect()
}

impl std::fmt::Debug for McpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpConfig::Local {
                command,
                environment,
                cwd,
                disabled,
            } => f
                .debug_struct("Local")
                .field("command", command)
                .field("environment", &names(environment))
                .field("cwd", cwd)
                .field("disabled", disabled)
                .finish(),
            McpConfig::Remote { url, headers, disabled } => f
                .debug_struct("Remote")
                .field("url", url)
                .field("headers", &names(headers))
                .field("disabled", disabled)
                .finish(),
        }
    }
}

impl McpConfig {
    /// The request's body.
    pub(crate) fn body(&self) -> Value {
        let config = match self {
            McpConfig::Local {
                command,
                environment,
                cwd,
                disabled,
            } => {
                let mut config = json!({"type": "local", "command": command, "disabled": disabled});
                if !environment.is_empty() {
                    config["environment"] = pairs(environment);
                }
                if let Some(cwd) = cwd {
                    config["cwd"] = json!(cwd.to_string_lossy());
                }
                config
            }
            McpConfig::Remote { url, headers, disabled } => {
                let mut config = json!({"type": "remote", "url": url, "disabled": disabled});
                if !headers.is_empty() {
                    config["headers"] = pairs(headers);
                }
                config
            }
        };
        json!({"config": config})
    }
}

/// Which stored sessions `Session::sessions` lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionQuery {
    /// Only sessions of this folder.
    pub directory: Option<PathBuf>,
    /// Only sessions whose title contains this.
    pub search: Option<String>,
    /// At most this many; OpenCode's default is 50.
    pub limit: Option<u32>,
    /// Where the previous page ended.
    pub cursor: Option<String>,
    /// Archived sessions instead of the others.
    pub archived: bool,
    /// Only sessions no other one started, leaving subagents' out.
    pub roots: bool,
}

impl SessionQuery {
    /// The query string of `GET /api/session`, with its `?` when not empty.
    pub(crate) fn query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(cursor) = &self.cursor {
            parts.push(format!("cursor={}", encode(cursor)));
        } else {
            parts.push("order=desc".to_string());
        }
        if let Some(directory) = &self.directory {
            parts.push(format!("directory={}", encode(&directory.to_string_lossy())));
        }
        if let Some(search) = &self.search {
            parts.push(format!("search={}", encode(search)));
        }
        if let Some(limit) = self.limit {
            parts.push(format!("limit={limit}"));
        }
        if self.archived {
            parts.push("archived=true".into());
        }
        if self.roots {
            parts.push("parentID=null".into());
        }
        format!("?{}", parts.join("&"))
    }
}

/// A stored session as a `Session.Info` describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    /// OpenCode's id, which resuming names.
    pub id: String,
    /// Its title, if it has one.
    pub title: Option<String>,
    /// The session it was started from, for a subagent's or a fork.
    pub parent: Option<String>,
    /// The folder it works in.
    pub directory: PathBuf,
    /// When it was created.
    pub created: Option<SystemTime>,
    /// When it changed last.
    pub updated: Option<SystemTime>,
    /// Its agent.
    pub agent: Option<String>,
    /// Its model.
    pub model: Option<ModelRef>,
    /// What it cost so far, in US dollars.
    pub cost: f64,
}

impl SessionSummary {
    pub(crate) fn from_json(info: &Value) -> Option<SessionSummary> {
        let times = info.get("time");
        Some(SessionSummary {
            id: text(info, "id")?,
            title: text(info, "title").filter(|t| !t.is_empty()),
            parent: text(info, "parentID").or_else(|| info.get("fork").and_then(|f| text(f, "sessionID"))),
            directory: info
                .get("location")
                .and_then(|l| text(l, "directory"))
                .map(PathBuf::from)
                .unwrap_or_default(),
            created: time(times, "created"),
            updated: time(times, "updated"),
            agent: text(info, "agent"),
            model: info.get("model").and_then(ModelRef::from_json),
            cost: info.get("cost").and_then(Value::as_f64).unwrap_or_default(),
        })
    }
}

pub(crate) fn session(result: Value) -> Result<SessionSummary, SessionError> {
    result
        .get("data")
        .and_then(SessionSummary::from_json)
        .ok_or_else(|| malformed("the session"))
}

/// One page of stored sessions.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPage {
    /// The sessions.
    pub sessions: Vec<SessionSummary>,
    /// Where the next page starts; `None` on the last.
    pub next_cursor: Option<String>,
}

pub(crate) fn session_page(result: Value) -> Result<SessionPage, SessionError> {
    let list = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("session.list"))?;
    Ok(SessionPage {
        sessions: list.iter().filter_map(SessionSummary::from_json).collect(),
        next_cursor: result.get("cursor").and_then(|c| text(c, "next")),
    })
}

/// A changed file a revert takes back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// The file.
    pub file: String,
    /// Its unified diff.
    pub patch: String,
    /// The lines added.
    pub additions: u64,
    /// The lines removed.
    pub deletions: u64,
    /// `added`, `deleted` or `modified`.
    pub status: String,
}

/// A staged revert: the conversation from `message_id` on and the files it changed are taken back once committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revert {
    /// The first message taken back.
    pub message_id: String,
    /// The files that change back.
    pub files: Vec<FileChange>,
}

pub(crate) fn revert(result: Value) -> Result<Revert, SessionError> {
    let data = result.get("data").ok_or_else(|| malformed("session.revert.stage"))?;
    Ok(Revert {
        message_id: text(data, "messageID").ok_or_else(|| malformed("session.revert.stage"))?,
        files: data
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|f| {
                Some(FileChange {
                    file: text(f, "file")?,
                    patch: text(f, "patch").unwrap_or_default(),
                    additions: f.get("additions").and_then(Value::as_u64).unwrap_or_default(),
                    deletions: f.get("deletions").and_then(Value::as_u64).unwrap_or_default(),
                    status: text(f, "status").unwrap_or_default(),
                })
            })
            .collect(),
    })
}

/// How a credential signs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// An API key.
    Key,
    /// An OAuth sign-in, such as a subscription's.
    OAuth,
    /// Something outside OpenCode, such as a cloud's own sign-in.
    External,
}

/// A stored credential, without its key or tokens, which never leave the crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// OpenCode's id of it.
    pub id: String,
    /// What it signs in to, such as `anthropic`.
    pub integration: String,
    /// What the user called it.
    pub label: String,
    /// Whether it is the one in use for its integration.
    pub active: bool,
    /// How it signs in.
    pub kind: CredentialKind,
}

pub(crate) fn credentials(result: Value) -> Result<Vec<Credential>, SessionError> {
    let list = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("credential.list"))?;
    Ok(list
        .iter()
        .filter_map(|entry| {
            Some(Credential {
                id: text(entry, "id")?,
                integration: text(entry, "integrationID")?,
                label: text(entry, "label").unwrap_or_default(),
                active: entry.get("active").and_then(Value::as_bool).unwrap_or(false),
                kind: match entry.get("value").and_then(|v| v.get("type")).and_then(Value::as_str)? {
                    "key" => CredentialKind::Key,
                    "oauth" => CredentialKind::OAuth,
                    _ => CredentialKind::External,
                },
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn answers_of_the_management_calls_read_into_their_types() {
        let servers = mcp_servers(json!({"location": {"directory": "/w"}, "data": [
            {"name": "docs", "status": {"status": "needs_auth", "error": "sign in"}},
            {"name": "fs", "title": "Files", "status": {"status": "connected"}}]}))
        .unwrap();
        assert_eq!(servers[0].status, McpStatus::NeedsAuth("sign in".into()));
        assert_eq!(servers[1].title.as_deref(), Some("Files"));
        let page = session_page(json!({"data": [{"id": "ses_1", "projectID": "p", "cost": 0.5,
            "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
            "time": {"created": 1000, "updated": 2000}, "title": "Fix it", "location": {"directory": "/w"},
            "model": {"providerID": "anthropic", "id": "claude"}}], "cursor": {"next": "c2", "previous": null}}))
        .unwrap();
        assert_eq!(page.sessions[0].title.as_deref(), Some("Fix it"));
        assert_eq!(page.sessions[0].created, Some(UNIX_EPOCH + Duration::from_secs(1)));
        assert_eq!(page.next_cursor.as_deref(), Some("c2"));
        let creds = credentials(
            json!({"data": [{"id": "c1", "integrationID": "anthropic", "label": "Work",
            "active": true, "value": {"type": "key", "key": "sk-secret"}}]}),
        )
        .unwrap();
        assert_eq!(creds[0].kind, CredentialKind::Key);
        assert!(!format!("{creds:?}").contains("sk-secret"));
        assert!(credentials(json!({})).is_err());
    }

    #[test]
    fn queries_and_configurations_go_out_as_the_schema_names_them() {
        let query = SessionQuery {
            directory: Some("/w x".into()),
            limit: Some(20),
            roots: true,
            ..SessionQuery::default()
        };
        assert_eq!(query.query(), "?order=desc&directory=%2Fw%20x&limit=20&parentID=null");
        let next = SessionQuery {
            cursor: Some("c2".into()),
            ..SessionQuery::default()
        };
        assert_eq!(next.query(), "?cursor=c2");
        let local = McpConfig::Local {
            command: vec!["npx".into(), "server".into()],
            environment: vec![("TOKEN".into(), "t".into())],
            cwd: None,
            disabled: false,
        };
        assert_eq!(
            local.body(),
            json!({"config": {"type": "local", "command": ["npx", "server"], "disabled": false,
                              "environment": {"TOKEN": "t"}}})
        );
        let remote = McpConfig::Remote {
            url: "https://mcp.example.test".into(),
            headers: Vec::new(),
            disabled: true,
        };
        assert_eq!(remote.body()["config"]["type"], "remote");
        let shown = format!("{local:?}");
        assert!(shown.contains("TOKEN") && !shown.contains("\"t\""), "{shown}");
        let remote = McpConfig::Remote {
            url: "https://mcp.example.test".into(),
            headers: vec![("Authorization".into(), "Bearer sk-secret".into())],
            disabled: false,
        };
        assert!(!format!("{remote:?}").contains("sk-secret"));
    }
}
