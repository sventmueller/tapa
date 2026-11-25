//! OpenCode over its own v2 HTTP API (E147, "SDK first, ACP as fallback"): `opencode serve --stdio` on the loopback
//! interface, a session that fills the shared thread model like an ACP session does, and the management calls
//! OpenCode offers beyond ACP. The API is the one OpenCode 2.2.6 describes in its OpenAPI schema
//! (`tests/fixtures/openapi-2.2.6.json`); nothing of it leaves this crate but the thread model's types and the small
//! result types of [`manage`]. An older OpenCode is refused with [`SessionError::Unsupported`], so the app can use
//! `opencode acp` through `term-acp` instead.
//!
//! # Process and framing
//!
//! - [`Session::start`] spawns `opencode serve --stdio --port 0` through `term_process::Supervisor` as `Kind::Agent`,
//!   in the project folder, with the login shell's environment of [`Launch::env`] (E10). Despite its name, `--stdio`
//!   does not speak the API on stdio: the server binds a free port of 127.0.0.1, prints one JSON line
//!   `{"url":"http://127.0.0.1:<port>"}` on stdout and runs while its stdin stays open. The session keeps stdin open
//!   for its life; closing it ([`Session::stop`], or the supervisor's graceful step) ends the server, and the end of
//!   its stdout is how the session learns the process is gone.
//! - Every request is HTTP/1.1 to that address through the crate's own small client (`http.rs`): JSON bodies, `Connection: close`, chunked or sized answers. Addresses other than 127.0.0.1 are
//!   refused. Location-scoped operations get the project folder as `?location[directory]=…`, and a new session
//!   names it as its `location`.
//! - Errors come as JSON with a `_tag` and a `message` ([`ServerError`]).
//!
//! # Authentication
//!
//! Each start makes a random password of 32 bytes from `/dev/urandom` and hands it to the server as
//! `OPENCODE_SERVER_PASSWORD`; `OPENCODE_PASSWORD`, which OpenCode would prefer, and any password of the login shell
//! are removed from the child's environment. Every request carries HTTP basic auth with the user `opencode` and that
//! password; without it the server answers 401. The password never enters a log, the process label or `Debug`
//! output, and the server removes it from the environment of the tools it runs.
//!
//! # Handshake
//!
//! On the thread `opencode-open`, in this order:
//!
//! 1. `opencode --version` through the supervisor ([`version::probe`]); below [`TESTED`] (2.2.6) the session reports
//!    `Unsupported` and stops the server.
//! 2. The address line on stdout (30 s at most).
//! 3. `GET /api/info` for the server's release.
//! 4. `GET /api/event`, the event stream, before anything that has events, read from then on by `opencode-events`.
//! 5. `GET /api/agent`, `/api/model`, `/api/model/default`, `/api/provider` for the project folder.
//! 6. `POST /api/session` with the folder, [`Launch::agent`] and [`Launch::model`]; or for [`Launch::resume`] (an
//!    OpenCode session id, `ses_…`) `GET /api/session/{id}` and every page of `GET /api/session/{id}/message`, which
//!    become the thread's history, then the permissions (`GET /api/permission/request`) and forms (`GET
//!    /api/session/{id}/form`) that still wait.
//! 7. Without a default model and without models the session reports [`SessionError::SignIn`]: no provider is
//!    connected.
//!
//! # Turns and settings
//!
//! | Dango | OpenCode |
//! |---|---|
//! | [`Session::send`] | `POST /api/session/{id}/prompt` with `text`, images as `files` of `data:` URIs; while a turn runs with `delivery: "steer"` |
//! | [`Session::cancel`] | open requests refused, then `POST /api/session/{id}/interrupt` |
//! | Mode ([`Session::set_mode`], option `mode`) | the agent: `POST /api/session/{id}/agent`; the modes are the agents that are neither subagents nor hidden, such as `build` and `plan`, the ids `opencode acp` uses |
//! | Option `model` (`provider/model`) | `POST /api/session/{id}/model` with a `Model.Ref` |
//! | Option `effort` (also `variant`) | the model's variant, the same call |
//! | [`Session::answer`] | `POST /api/session/{id}/permission/{per}/reply` with `once`, `always` or `reject`; a form's `Dismiss` is `DELETE /api/session/{id}/form/{frm}` |
//! | [`Session::answer_questions`], [`Session::answer_form`] | `POST /api/session/{id}/form/{frm}/reply` |
//!
//! Settings are shown at once and taken back with a notice if the server refuses them.
//!
//! # Events and the thread
//!
//! The stream is server-sent events, each `data` one `V2Event` (`{id, created, type, data, …}`, `created` in
//! milliseconds). Events of other sessions (subagents, other clients) are left out by `data.sessionID`. Keys are
//! `text:{message}:{ordinal}`, `reasoning:{message}:{ordinal}` and `tool:{message}:{call}`.
//!
//! | Event | Thread |
//! |---|---|
//! | `session.execution.started` | status running; the turn's tally starts |
//! | `session.execution.succeeded`, `session.idle` | open text ends, the turn's tokens, cost and duration, status idle |
//! | `session.execution.failed` | an error notice (unless a step's came first), then as succeeded |
//! | `session.execution.interrupted` | an interrupted notice for `user`, an info notice for another reason, then idle |
//! | `session.step.started` / `.streamed` | activity "requesting" / none |
//! | `session.step.ended` | context used: the step's input, output, reasoning and cache tokens |
//! | `session.step.failed` | an error notice |
//! | `session.text.started` / `.delta` / `.ended` | an agent message, streaming, finished with the full text |
//! | `session.reasoning.started` / `.delta` / `.ended` | a thought, likewise, with its duration |
//! | `session.tool.input.started` | a pending tool card named as Claude Code names the tool |
//! | `session.tool.called` | the card's input, title and view, in progress; `todowrite` sets the to-do list instead, without a card, as Claude Code's `TodoWrite` shows |
//! | `session.tool.progress` | a command's output so far (`metadata.output`) |
//! | `session.tool.success` / `.failed` | completed with its output (terminal, diff from `metadata.files`, or text) / failed with the error |
//! | `session.retry.scheduled` | a warning notice per assistant message |
//! | `session.compaction.started` / `.ended` / `.failed` | activity "compacting" / a compaction notice / an error notice |
//! | `session.usage.updated` | the session's tokens and cost in US dollars |
//! | `session.renamed` | the title |
//! | `session.agent.selected`, `session.model.selected` | mode, model and effort, and the capabilities |
//! | `permission.asked` | a permission request (`once`, `always` with the request's `save` patterns as the rule, `reject`; no `always` for a git command that destroys work, E27), its tool card pending, status waiting |
//! | `permission.replied`, `form.replied`, `form.cancelled` | a request answered elsewhere is resolved |
//! | `form.created` | a question entry, one item per visible field, options from the field's options |
//! | `mcp.status.changed` | [`Event::McpChanged`] |
//! | `agent.updated`, `model.updated`, `provider.updated`, credential and integration events | the offers are read again |
//!
//! Tool names: `shell`/`bash` → `Bash`, `read` → `Read`, `write` → `Write`, `edit` and `patch` → `Edit`, `glob` →
//! `Glob`, `grep` → `Grep`, `webfetch` → `WebFetch`, `websearch` → `WebSearch`, `subagent`/`task` → `Task`,
//! `question` → `AskUserQuestion`, `skill` → `Skill`; others keep their name.
//!
//! The stream is volatile by the schema's contract: a slow reader loses it and events while it is down are missed.
//! When it breaks while the server runs, it is opened again up to five times before the session reports the failure
//! and stops the server.
//!
//! # Management calls
//!
//! [`Session::info`], [`Session::models`], [`Session::agents`], [`Session::providers`]; MCP servers with
//! [`Session::mcp_servers`] (`GET /api/mcp`), [`Session::add_mcp`] and [`Session::remove_mcp`] (`PUT`/`DELETE
//! /api/experimental/mcp/{server}`), [`Session::connect_mcp`] and [`Session::disconnect_mcp`]; stored sessions with
//! [`Session::sessions`]; [`Session::fork`]; [`Session::export`]; reverting with [`Session::revert_stage`],
//! [`Session::revert_commit`] and [`Session::revert_clear`]; [`Session::credentials`] without their secrets;
//! [`Session::rate_limits`] for an OpenCode Go subscription; and [`Session::request`] for any other operation.
//!
//! # Testing
//!
//! [`test_server`] serves the subset of the API this crate uses on 127.0.0.1 with the same authentication and
//! address line, and plays fixed turns; the binary `opencode-test-server` is it as an `opencode` executable.

#![forbid(unsafe_code)]

pub mod capabilities;
mod diff;
mod http;
pub mod launch;
pub mod manage;
pub mod mapping;
mod prompt;
mod requests;
pub mod session;
#[cfg(any(test, feature = "testing"))]
pub mod test_server;
pub mod version;

pub use capabilities::{Agent, AgentMode, Capabilities, Model, ModelRef, Provider};
pub use http::HttpError;
pub use launch::Launch;
pub use manage::{
    Credential, CredentialKind, FileChange, McpConfig, McpServer, McpStatus, Revert, ServerInfo, SessionPage,
    SessionQuery, SessionSummary,
};
pub use mapping::Mapper;
pub use session::{Choice, Event, Reply, ServerError, Session, SessionError};
pub use version::{TESTED, Version};

/// The id of the HTTP connection on its threads; the ACP route's preset is `opencode`.
pub const AGENT: &str = "opencode";
