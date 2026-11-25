//! The management calls of a session: the server, its agents, models and providers, MCP servers, stored sessions,
//! forking, exporting, reverting, the credentials and the usage limits. Each answers through a `Reply`.

use serde_json::{Value, json};
use term_thread::{RateLimit, Update};

use super::{After, Event, Reply, Session, SessionError, now, raw, unit};
use crate::capabilities::{self, Agent, Model, Provider};
use crate::http::encode;
use crate::manage::{
    self, Credential, McpConfig, McpServer, Revert, ServerInfo, SessionPage, SessionQuery, SessionSummary,
};

fn models(result: Value) -> Result<Vec<Model>, SessionError> {
    Ok(capabilities::list(&result, Model::from_json))
}

fn agents(result: Value) -> Result<Vec<Agent>, SessionError> {
    Ok(capabilities::list(&result, Agent::from_json))
}

fn providers(result: Value) -> Result<Vec<Provider>, SessionError> {
    Ok(capabilities::list(&result, Provider::from_json))
}

fn limits(result: Value) -> Result<Vec<RateLimit>, SessionError> {
    let info = result.get("data").unwrap_or(&Value::Null);
    Ok(crate::mapping::rate_limits(info, now())
        .into_iter()
        .filter_map(|u| match u {
            Update::RateLimit(limit) => Some(limit),
            _ => None,
        })
        .collect())
}

impl Session {
    fn session_call<T: Send + 'static>(
        &self,
        method: &'static str,
        rest: &str,
        body: Option<Value>,
        read: super::reply::Read<T>,
    ) -> Reply<T> {
        match self.shared.session_path(rest) {
            Ok(path) => self.shared.call(method, path, body, None, read),
            Err(error) => Reply::failed(error, read),
        }
    }

    /// Sends `method` to `target`, a path with its query such as `/api/vcs/status`, with `body` as JSON, for what
    /// the typed calls do not cover.
    pub fn request(&self, method: &'static str, target: &str, body: Option<Value>) -> Reply<Value> {
        self.shared.call(method, target.to_string(), body, None, raw)
    }

    /// What the server says about itself.
    pub fn info(&self) -> Reply<ServerInfo> {
        self.shared
            .call("GET", "/api/info".into(), None, None, manage::server_info)
    }

    /// The models of the project folder, which the capabilities then list too.
    pub fn models(&self) -> Reply<Vec<Model>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                shared.state().caps.learn_models(result);
                shared.emit(Event::Capabilities);
            }
        });
        let target = format!("/api/model?{}", self.shared.location());
        self.shared.call("GET", target, None, Some(after), models)
    }

    /// The agents of the project folder, which the capabilities then list too.
    pub fn agents(&self) -> Reply<Vec<Agent>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                shared.state().caps.learn_agents(result);
                shared.emit(Event::Capabilities);
            }
        });
        let target = format!("/api/agent?{}", self.shared.location());
        self.shared.call("GET", target, None, Some(after), agents)
    }

    /// The providers of the project folder, which the capabilities then list too.
    pub fn providers(&self) -> Reply<Vec<Provider>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                shared.state().caps.learn_providers(result);
                shared.emit(Event::Capabilities);
            }
        });
        let target = format!("/api/provider?{}", self.shared.location());
        self.shared.call("GET", target, None, Some(after), providers)
    }

    /// The MCP servers with their state.
    pub fn mcp_servers(&self) -> Reply<Vec<McpServer>> {
        let target = format!("/api/mcp?{}", self.shared.location());
        self.shared.call("GET", target, None, None, manage::mcp_servers)
    }

    fn mcp_call(&self, method: &'static str, name: &str, rest: &str, body: Option<Value>) -> Reply<()> {
        let target = format!(
            "/api/experimental/mcp/{}{rest}?{}",
            encode(name),
            self.shared.location()
        );
        self.shared.call(method, target, body, None, unit)
    }

    /// Adds the MCP server `name` to the project's configuration, or replaces it.
    pub fn add_mcp(&self, name: &str, config: &McpConfig) -> Reply<()> {
        self.mcp_call("PUT", name, "", Some(config.body()))
    }

    /// Removes the MCP server `name` from the configuration.
    pub fn remove_mcp(&self, name: &str) -> Reply<()> {
        self.mcp_call("DELETE", name, "", None)
    }

    /// Connects the MCP server `name`, which also starts its sign-in when it needs one.
    pub fn connect_mcp(&self, name: &str) -> Reply<()> {
        self.mcp_call("POST", name, "/connect", None)
    }

    /// Disconnects the MCP server `name`.
    pub fn disconnect_mcp(&self, name: &str) -> Reply<()> {
        self.mcp_call("POST", name, "/disconnect", None)
    }

    /// The stored sessions `query` selects.
    pub fn sessions(&self, query: &SessionQuery) -> Reply<SessionPage> {
        let target = format!("/api/session{}", query.query());
        self.shared.call("GET", target, None, None, manage::session_page)
    }

    /// Copies this session into a new stored one, up to the message `before` when given; resuming the copy is a
    /// session of its own.
    pub fn fork(&self, before: Option<&str>) -> Reply<SessionSummary> {
        let body = before.map_or_else(|| json!({}), |id| json!({"before": id}));
        self.session_call("POST", "/fork", Some(body), manage::session)
    }

    /// The whole session as OpenCode exports it, its secrets left out with `sanitize`.
    pub fn export(&self, sanitize: bool) -> Reply<Value> {
        match self.session_id().filter(|_| !self.is_ended()) {
            Some(id) => {
                let target = format!("/api/experimental/session/{}/export?sanitize={sanitize}", encode(&id));
                self.shared.call("GET", target, None, None, raw)
            }
            None => Reply::failed(SessionError::Closed, raw),
        }
    }

    /// Stages taking back the conversation from the message `message_id` on, and with `files` what it changed on
    /// disk; `revert_commit` makes it final, `revert_clear` drops it.
    pub fn revert_stage(&self, message_id: &str, files: bool) -> Reply<Revert> {
        let body = json!({"messageID": message_id, "files": files});
        self.session_call("POST", "/revert/stage", Some(body), manage::revert)
    }

    /// Makes the staged revert final.
    pub fn revert_commit(&self) -> Reply<()> {
        self.session_call("POST", "/revert/commit", None, unit)
    }

    /// Drops the staged revert.
    pub fn revert_clear(&self) -> Reply<()> {
        self.session_call("DELETE", "/revert", None, unit)
    }

    /// The stored credentials, without their keys and tokens.
    pub fn credentials(&self) -> Reply<Vec<Credential>> {
        self.shared
            .call("GET", "/api/credential".into(), None, None, manage::credentials)
    }

    /// The usage windows of an OpenCode Go subscription, which the thread then shows too; empty without one.
    pub fn rate_limits(&self) -> Reply<Vec<RateLimit>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result
                && let Some(info) = result.get("data").filter(|d| d.is_object())
            {
                let mut updates = crate::mapping::rate_limits(info, now());
                if info.get("go").is_some_and(Value::is_object) {
                    updates.push(Update::Usage(term_thread::UsagePatch {
                        subscription: Some(true),
                        ..term_thread::UsagePatch::default()
                    }));
                }
                shared.apply(updates);
            }
        });
        let target = format!("/api/experimental/usage/limits?{}", self.shared.location());
        self.shared.call("GET", target, None, Some(after), limits)
    }
}
