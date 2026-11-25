//! The commands that open an overlay instead of going to the agent, offered in the `/` menu and while typing
//! for the agents whose backend can do them.

use splat_i18n::t;

use crate::acp::Agent;

/// The overlays' commands `agent` offers, by name without the slash, each with what it does; for an ACP agent those
/// its capabilities `acp` allow.
pub(crate) fn local_commands(
    agent: &Agent,
    acp: Option<&term_acp::Capabilities>,
    codex: bool,
) -> Vec<(String, String)> {
    let claude = [
        ("status", t("Zeigt, womit die Sitzung läuft")),
        ("export", t("Kopiert oder sichert die Unterhaltung")),
        ("mcp", t("MCP-Server einrichten, anmelden und schalten")),
        ("permissions", t("Regeln, was der Agent ohne Frage darf")),
        ("hooks", t("Befehle, die bei Ereignissen laufen")),
        ("memory", t("Anweisungsdateien und automatisches Gedächtnis")),
        ("config", t("Einstellungen und Ausgabestil des Agenten")),
        ("output-style", t("Ausgabestil wählen")),
        ("skills", t("Skills ein- und ausschalten")),
        ("plugins", t("Plugins und Marktplätze verwalten")),
        ("logout", t("Wer angemeldet ist, und abmelden")),
        (
            "bug",
            if codex {
                t("Einen Fehler oder eine Idee an OpenAI melden")
            } else {
                t("Einen Fehler oder eine Idee an Anthropic melden")
            },
        ),
        ("btw", t("Eine Frage nebenbei, die nicht in die Unterhaltung kommt")),
        ("resume", t("Setzt eine frühere Sitzung dieses Ordners fort")),
    ];
    let offered: Vec<&str> = match agent {
        Agent::Claude => claude.iter().map(|(name, _)| *name).collect(),
        _ if codex => vec![
            "status", "export", "mcp", "memory", "config", "skills", "logout", "bug", "resume",
        ],
        _ => {
            let caps = acp.cloned().unwrap_or_default();
            let mut names = vec!["status", "export", "mcp", "memory", "config"];
            if caps.logout {
                names.push("logout");
            }
            if caps.fork {
                names.push("btw");
            }
            if caps.load_session {
                names.push("resume");
            }
            names
        }
    };
    claude
        .into_iter()
        .filter(|(name, _)| offered.contains(name))
        .map(|(name, what)| (name.to_string(), what.to_string()))
        .collect()
}

/// `commands` with the overlays' commands of `agent` added where the agent did not list them itself.
pub(crate) fn with_local(
    mut commands: Vec<(String, String)>,
    agent: &Agent,
    acp: Option<&term_acp::Capabilities>,
    codex: bool,
) -> Vec<(String, String)> {
    for (name, what) in local_commands(agent, acp, codex) {
        if !commands.iter().any(|(known, _)| *known == name) {
            commands.push((name, what));
        }
    }
    commands
}
