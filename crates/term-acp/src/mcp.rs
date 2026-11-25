//! The MCP servers the app hands an ACP agent when its session starts, only in a trusted workspace (E16).

use serde::{Deserialize, Serialize};

/// How the agent reaches a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// A program the agent starts.
    Stdio,
    /// Streamable HTTP.
    Http,
    /// Server-sent events.
    Sse,
}

/// Where a server is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOrigin {
    /// The project's `.mcp.json`.
    Project,
    /// The user's own list in the app.
    User,
}

/// A server as it is handed over: names and values resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    /// Its name.
    pub name: String,
    /// How the agent reaches it.
    pub transport: McpTransport,
    /// The program, for `Stdio`.
    pub command: String,
    /// The program's arguments.
    pub args: Vec<String>,
    /// The address, for `Http` and `Sse`.
    pub url: String,
    /// The program's environment.
    pub env: Vec<(String, String)>,
    /// The HTTP headers.
    pub headers: Vec<(String, String)>,
    /// Where it is defined.
    pub origin: McpOrigin,
    /// Whether it is handed over.
    pub enabled: bool,
}
