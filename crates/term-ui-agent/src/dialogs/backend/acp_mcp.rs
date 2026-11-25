//! The MCP servers an ACP agent gets when its session starts: those of the project's `.mcp.json`, switched per
//! project, and only in a trusted workspace (E16). The app keeps no list of the user's own servers, since their
//! values are credentials it does not store (E21).

use std::path::{Path, PathBuf};

use serde_json::Value;
use term_acp::mcp::{McpOrigin, McpServerSpec, McpTransport};
use term_base::explain::Explained;

use crate::dialogs::model::Scope;

/// Every server of `project` the agent gets, also those switched off, with `${VAR}` filled from `env`; none where
/// the workspace is not trusted.
pub(crate) fn servers(project: &Path, env: &[(String, String)], trusted: bool) -> Vec<McpServerSpec> {
    if !trusted {
        return Vec::new();
    }
    let off = switched_off_list();
    project_servers(project, env)
        .into_iter()
        .map(|mut server| {
            server.enabled = !switched_off(&off, project, &server.name);
            server
        })
        .collect()
}

/// Switches the project's server `name` on or off.
pub(crate) fn toggle(project: &Path, name: &str, enabled: bool) -> Result<(), Explained> {
    let mut off = switched_off_list();
    switch(&mut off, project, name, enabled);
    term_base::atomic_write::write_json(&off_file(), &off).map_err(|e| Explained::of(&e))
}

/// The scope `server` shows under.
pub(crate) fn scope(server: &McpServerSpec) -> Scope {
    match server.origin {
        McpOrigin::Project => Scope::Project,
        McpOrigin::User => Scope::User,
    }
}

/// The file of the projects' servers the user switched off, as `<project>\n<name>`.
fn off_file() -> PathBuf {
    term_base::data_dir().join("acp-mcp-off.json")
}

fn switched_off_list() -> Vec<String> {
    std::fs::read(off_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Whether the user switched off the project server `name` of `project`.
fn switched_off(off: &[String], project: &Path, name: &str) -> bool {
    off.contains(&format!("{}\n{name}", project.display()))
}

/// `off` with the project server `name` of `project` switched on or off.
fn switch(off: &mut Vec<String>, project: &Path, name: &str, on: bool) {
    let entry = format!("{}\n{name}", project.display());
    off.retain(|e| *e != entry);
    if !on {
        off.push(entry);
    }
}

/// The servers of `project`'s `.mcp.json`, with `${VAR}` and `${VAR:-default}` filled from `env`; none when it is
/// missing or unreadable.
fn project_servers(project: &Path, env: &[(String, String)]) -> Vec<McpServerSpec> {
    let Ok(text) = std::fs::read_to_string(project.join(".mcp.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    json.get("mcpServers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, server)| project_server(name, server, env))
        .collect()
}

fn project_server(name: &str, server: &Value, env: &[(String, String)]) -> McpServerSpec {
    let text = |key: &str| expand(server.get(key).and_then(Value::as_str).unwrap_or_default(), env);
    let pairs = |key: &str| -> Vec<(String, String)> {
        let mut pairs: Vec<(String, String)> = server
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), expand(v.as_str().unwrap_or_default(), env)))
            .collect();
        pairs.sort();
        pairs
    };
    let transport = match server.get("type").and_then(Value::as_str) {
        Some("http") => McpTransport::Http,
        Some("sse") => McpTransport::Sse,
        _ if server.get("url").is_some() && server.get("command").is_none() => McpTransport::Http,
        _ => McpTransport::Stdio,
    };
    McpServerSpec {
        name: name.to_string(),
        transport,
        command: text("command"),
        args: server
            .get("args")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|arg| expand(arg, env))
            .collect(),
        url: text("url"),
        env: pairs("env"),
        headers: pairs("headers"),
        origin: McpOrigin::Project,
        enabled: true,
    }
}

/// `text` with `${VAR}` and `${VAR:-default}` filled from `env`, as Claude Code reads `.mcp.json`.
fn expand(text: &str, env: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &after[..end];
        let (name, default) = inner.split_once(":-").map_or((inner, None), |(n, d)| (n, Some(d)));
        match env.iter().find(|(k, _)| k == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(default.unwrap_or_default()),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_server_is_switched_per_project() {
        let mut off = Vec::new();
        switch(&mut off, Path::new("/p"), "docs", false);
        assert!(switched_off(&off, Path::new("/p"), "docs"));
        assert!(!switched_off(&off, Path::new("/q"), "docs"), "another project keeps it");
        switch(&mut off, Path::new("/p"), "docs", true);
        assert!(off.is_empty());
    }

    #[test]
    fn the_project_file_fills_its_variables_and_only_a_trusted_workspace_gets_it() {
        let dir = term_base::testing::scratch("acp-mcp");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"web":{"url":"https://${HOST:-example.org}/mcp"},"run":{"command":"${BIN}","args":["-v"]}}}"#,
        )
        .unwrap();
        let env = [("BIN".to_string(), "/usr/bin/tool".to_string())];
        let found = project_servers(&dir, &env);
        let web = found.iter().find(|s| s.name == "web").unwrap();
        assert_eq!(
            (web.transport, web.url.as_str()),
            (McpTransport::Http, "https://example.org/mcp")
        );
        let run = found.iter().find(|s| s.name == "run").unwrap();
        assert_eq!(
            (run.command.as_str(), run.args.clone()),
            ("/usr/bin/tool", vec!["-v".to_string()])
        );
        assert!(
            servers(&dir, &env, false).is_empty(),
            "an untrusted workspace hands no project server over"
        );
    }
}
