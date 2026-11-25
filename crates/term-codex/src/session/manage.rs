//! The management calls of a session: stored threads, models, configuration, MCP servers, skills, the account,
//! usage limits, feedback and plugins. Each answers through a `Reply`.

use std::path::Path;

use serde_json::{Value, json};
use term_thread::{RateLimit, Update};

use super::{After, Event, Reply, Session, SessionError, now, reply};
use crate::capabilities::Model;
use crate::manage::{
    self, AccountState, ConfigEdit, ConfigWritten, Feedback, FeedbackSent, Forked, Login, LoginStarted, McpLogin,
    McpServer, MergeStrategy, Skills, ThreadPage, ThreadQuery, ThreadSummary,
};
use crate::protocol::method;

fn models(result: Value) -> Result<Vec<Model>, SessionError> {
    reply::typed(result.get("data").cloned().unwrap_or(Value::Null))
}

fn forked(result: Value) -> Result<ThreadSummary, SessionError> {
    reply::typed::<Forked>(result).map(|f| f.thread)
}

fn limits(result: Value) -> Result<Vec<RateLimit>, SessionError> {
    let snapshot = result.get("rateLimits").unwrap_or(&Value::Null);
    Ok(crate::mapping::rate_limits(snapshot, now())
        .into_iter()
        .filter_map(|u| match u {
            Update::RateLimit(limit) => Some(limit),
            _ => None,
        })
        .collect())
}

impl Session {
    /// Sends the request `method` with `params`, for what the typed calls do not cover.
    pub fn request(&self, method: &str, params: Value) -> Reply<Value> {
        self.shared.call(method, params, None, reply::raw)
    }

    /// The stored threads `query` selects.
    pub fn threads(&self, query: &ThreadQuery) -> Reply<ThreadPage> {
        self.shared
            .call(method::THREAD_LIST, query.params(), None, reply::typed)
    }

    /// Copies this thread into a new stored one, up to and including `last_turn` when given; resuming the copy is a
    /// session of its own.
    pub fn fork(&self, last_turn: Option<&str>) -> Result<Reply<ThreadSummary>, SessionError> {
        let thread = self.thread_id()?;
        let mut params = json!({"threadId": thread, "excludeTurns": true});
        if let Some(turn) = last_turn {
            params["lastTurnId"] = json!(turn);
        }
        Ok(self.shared.call(method::THREAD_FORK, params, None, forked))
    }

    /// Drops the turn `before_turn` and every later one from the stored thread, so resuming it continues before
    /// them; local files stay as they are.
    pub fn revert(&self, before_turn: &str) -> Result<Reply<()>, SessionError> {
        let thread = self.thread_id()?;
        let params = json!({"threadId": thread, "beforeTurnId": before_turn});
        Ok(self.shared.call(method::THREAD_REVERT, params, None, reply::unit))
    }

    /// The models, which the capabilities then list too.
    pub fn models(&self, include_hidden: bool) -> Reply<Vec<Model>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                shared.state().caps.learn_models(result);
                shared.emit(Event::Capabilities);
            }
        });
        let params = json!({"includeHidden": include_hidden});
        self.shared.call(method::MODEL_LIST, params, Some(after), models)
    }

    /// The effective configuration for the project folder, with the layers it comes from when `layers`.
    pub fn read_config(&self, layers: bool) -> Reply<Value> {
        let params = json!({"cwd": self.shared.launch.cwd.to_string_lossy(), "includeLayers": layers});
        self.shared.call(method::CONFIG_READ, params, None, reply::raw)
    }

    /// Writes `value` at `key_path` of the user's `config.toml`.
    pub fn write_config(&self, key_path: &str, value: Value, merge: MergeStrategy) -> Reply<ConfigWritten> {
        let params = json!({"keyPath": key_path, "value": value, "mergeStrategy": merge.wire()});
        self.shared.call(method::CONFIG_VALUE_WRITE, params, None, reply::typed)
    }

    /// Writes `edits` into the user's `config.toml` at once; with `reload`, loaded threads take up what changed.
    pub fn write_configs(&self, edits: &[ConfigEdit], reload: bool) -> Reply<ConfigWritten> {
        let edits: Vec<Value> = edits.iter().map(ConfigEdit::json).collect();
        let params = json!({"edits": edits, "reloadUserConfig": reload});
        self.shared.call(method::CONFIG_BATCH_WRITE, params, None, reply::typed)
    }

    /// The MCP servers with their sign-in state and tools.
    pub fn mcp_servers(&self) -> Reply<Vec<McpServer>> {
        let mut params = json!({});
        if let Some(thread) = self.session_id() {
            params["threadId"] = json!(thread);
        }
        self.shared
            .call(method::MCP_STATUS_LIST, params, None, manage::mcp_servers)
    }

    /// Starts the OAuth sign-in of the MCP server `name`; `Event::McpLogin` says when it ends.
    pub fn mcp_login(&self, name: &str) -> Reply<McpLogin> {
        self.shared
            .call(method::MCP_OAUTH_LOGIN, json!({"name": name}), None, reply::typed)
    }

    /// Reloads the MCP servers from the configuration.
    pub fn reload_mcp(&self) -> Reply<()> {
        self.shared.call(method::MCP_RELOAD, Value::Null, None, reply::unit)
    }

    /// The skills of the project folder; `reload` reads them from disk anew.
    pub fn skills(&self, reload: bool) -> Reply<Vec<Skills>> {
        let params = json!({"cwds": [self.shared.launch.cwd.to_string_lossy()], "forceReload": reload});
        self.shared.call(method::SKILLS_LIST, params, None, manage::skills)
    }

    /// Turns the skill at `path` on or off; the answer is whether it is on now.
    pub fn set_skill(&self, path: &Path, enabled: bool) -> Reply<bool> {
        let params = json!({"path": path.to_string_lossy(), "enabled": enabled});
        self.shared
            .call(method::SKILLS_CONFIG_WRITE, params, None, manage::skill_enabled)
    }

    /// The account, which the capabilities then hold too.
    pub fn account(&self) -> Reply<AccountState> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                let subscription = {
                    let mut state = shared.state();
                    state.caps.learn_account(result);
                    crate::mapping::usage_subscription(&state.caps)
                };
                shared.apply(vec![subscription]);
                shared.emit(Event::Capabilities);
            }
        });
        self.shared.call(
            method::ACCOUNT_READ,
            json!({"refreshToken": false}),
            Some(after),
            manage::account,
        )
    }

    /// Signs out.
    pub fn logout(&self) -> Reply<()> {
        let after: After = Box::new(|result, shared| {
            if result.is_ok() {
                shared.state().caps.account = None;
                shared.emit(Event::Capabilities);
            }
        });
        self.shared
            .call(method::ACCOUNT_LOGOUT, Value::Null, Some(after), reply::unit)
    }

    /// Starts a sign-in `how`; `Event::Login` says when it ends.
    pub fn login(&self, how: &Login) -> Reply<LoginStarted> {
        self.shared
            .call(method::ACCOUNT_LOGIN_START, how.params(), None, manage::login_started)
    }

    /// Stops the sign-in `login_id` that has not ended.
    pub fn cancel_login(&self, login_id: &str) -> Reply<()> {
        self.shared.call(
            method::ACCOUNT_LOGIN_CANCEL,
            json!({"loginId": login_id}),
            None,
            reply::unit,
        )
    }

    /// The plan's usage windows, which the thread then shows too.
    pub fn rate_limits(&self) -> Reply<Vec<RateLimit>> {
        let after: After = Box::new(|result, shared| {
            if let Ok(result) = result {
                let snapshot = result.get("rateLimits").unwrap_or(&Value::Null);
                shared.apply(crate::mapping::rate_limits(snapshot, now()));
            }
        });
        self.shared
            .call(method::ACCOUNT_RATE_LIMITS_READ, Value::Null, Some(after), limits)
    }

    /// Sends `feedback` about this thread to OpenAI; only on the user's explicit request.
    pub fn upload_feedback(&self, feedback: &Feedback) -> Reply<FeedbackSent> {
        let mut params = json!({"classification": feedback.classification, "includeLogs": feedback.include_logs});
        if let Some(reason) = &feedback.reason {
            params["reason"] = json!(reason);
        }
        if let Some(thread) = self.session_id() {
            params["threadId"] = json!(thread);
        }
        self.shared.call(method::FEEDBACK_UPLOAD, params, None, reply::typed)
    }

    /// The plugins of the marketplaces the project folder sees, as Codex reports them.
    pub fn plugins(&self) -> Reply<Value> {
        let params = json!({"cwds": [self.shared.launch.cwd.to_string_lossy()]});
        self.shared.call(method::PLUGIN_LIST, params, None, reply::raw)
    }
}
