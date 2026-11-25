//! `/plugins` for Claude Code: `claude plugin … --json` and `claude plugin marketplace … --json`, read into the
//! neutral model, and the running session reading its plugins again after a change.

use serde_json::Value;
use term_claude::Session;
use term_claude::cli::{Cli, CliError, PluginAnswer};

use crate::dialogs::model::{MarketChange, Marketplace, Plugin, PluginChange, PluginStep, Plugins, Scope};

/// The installed plugins, those the marketplaces offer and the marketplaces.
pub(super) fn list(cli: &Cli) -> Result<Plugins, CliError> {
    let plugins = answered(cli.plugin(&["list", "--available"])?)?;
    let markets = answered(cli.plugin(&["marketplace", "list"])?)?;
    Ok(read(&plugins, &markets))
}

/// Installs or updates `id`; asks first where the marketplace runs a command.
pub(super) fn install(
    cli: &Cli,
    session: Option<&Session>,
    id: &str,
    scope: Scope,
    update: bool,
    accept: Option<&str>,
) -> Result<PluginStep, CliError> {
    let mut args = vec![if update { "update" } else { "install" }, id, "-s", scope.id()];
    if let Some(sha) = accept {
        args.extend(["--accept-command", sha]);
    }
    let answer = cli.plugin(&args)?;
    if let Some(step) = confirm(&answer.json) {
        return Ok(step);
    }
    answered(answer)?;
    reload(session);
    Ok(PluginStep::Done)
}

/// Switches `id` on or off, or removes it.
pub(super) fn change(
    cli: &Cli,
    session: Option<&Session>,
    id: &str,
    scope: Scope,
    change: PluginChange,
) -> Result<(), CliError> {
    let verb = match change {
        PluginChange::Enable => "enable",
        PluginChange::Disable => "disable",
        PluginChange::Uninstall => "uninstall",
    };
    answered(cli.plugin(&[verb, id, "-s", scope.id()])?)?;
    reload(session);
    Ok(())
}

/// Adds, fetches again or removes a marketplace.
pub(super) fn market(cli: &Cli, session: Option<&Session>, name: &str, change: MarketChange) -> Result<(), CliError> {
    let verb = match change {
        MarketChange::Add => "add",
        MarketChange::Update => "update",
        MarketChange::Remove => "remove",
    };
    answered(cli.plugin(&["marketplace", verb, name])?)?;
    reload(session);
    Ok(())
}

fn reload(session: Option<&Session>) {
    if let Some(session) = session {
        // A plugin's commands and skills reach the session only once it reads its plugins again.
        let _ = session.reload_plugins().wait();
    }
}

/// The JSON of an answer that did what it was asked; else its own words.
fn answered(answer: PluginAnswer) -> Result<Value, CliError> {
    if answer.done {
        return Ok(answer.json);
    }
    let said = ["message", "error"]
        .iter()
        .find_map(|key| answer.json.get(key).and_then(Value::as_str))
        .map_or_else(|| answer.json.to_string(), str::to_string);
    Err(CliError::Failed(said))
}

/// The consent an answer asks for, when the marketplace declares a command for the plugin.
fn confirm(json: &Value) -> Option<PluginStep> {
    let shown = json.get("shownCommand")?;
    let sha256 = shown.get("sha256")?.as_str()?.to_string();
    let command = match shown.get("command") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    Some(PluginStep::Confirm { command, sha256 })
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// The answers of `plugin list --available` and `plugin marketplace list` in the neutral model.
pub(super) fn read(plugins: &Value, markets: &Value) -> Plugins {
    let installed: Vec<Plugin> = plugins["installed"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let id = text(p, "id");
            let (name, marketplace) = id
                .split_once('@')
                .map_or((id.clone(), String::new()), |(n, m)| (n.into(), m.into()));
            Plugin {
                id,
                name,
                marketplace,
                version: p.get("version").and_then(Value::as_str).map(str::to_string),
                scope: Some(Scope::from_id(&text(p, "scope")).unwrap_or(Scope::User)),
                enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                ..Plugin::default()
            }
        })
        .collect();
    let available = plugins["available"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| Plugin {
            id: text(p, "pluginId"),
            name: text(p, "name"),
            marketplace: text(p, "marketplaceName"),
            description: text(p, "description"),
            version: p.get("version").and_then(Value::as_str).map(str::to_string),
            installs: p.get("installCount").and_then(Value::as_u64),
            ..Plugin::default()
        })
        .collect();
    let marketplaces = markets
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| Marketplace {
            name: text(m, "name"),
            source: ["repo", "url", "path"]
                .iter()
                .find_map(|key| m.get(key).and_then(Value::as_str))
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Plugins {
        installed,
        available,
        marketplaces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_cli_lists_read_into_the_neutral_model() {
        let plugins = json!({
            "installed": [{"id": "demo@market", "version": "1.0.0", "scope": "project", "enabled": true}],
            "available": [{"pluginId": "lint@market", "name": "lint", "marketplaceName": "market",
                "description": "Lints.", "installCount": 12}]
        });
        let markets = json!([{"name": "market", "source": "github", "repo": "owner/market"}]);
        let read = read(&plugins, &markets);
        assert_eq!(read.installed[0].name, "demo");
        assert_eq!(read.installed[0].marketplace, "market");
        assert_eq!(read.installed[0].scope, Some(Scope::Project));
        assert!(read.installed[0].enabled);
        assert_eq!(read.available[0].installs, Some(12));
        assert_eq!(read.marketplaces[0].source, "owner/market");
    }

    #[test]
    fn a_declared_command_asks_for_consent_and_a_refusal_says_why() {
        let asking = json!({"ok": false, "shownCommand": {"command": ["make", "install"], "sha256": "ab"}});
        assert_eq!(
            confirm(&asking),
            Some(PluginStep::Confirm {
                command: "make install".into(),
                sha256: "ab".into()
            })
        );
        let refused = PluginAnswer {
            done: false,
            json: json!({"ok": false, "message": "not found"}),
        };
        assert_eq!(answered(refused), Err(CliError::Failed("not found".into())));
    }
}
