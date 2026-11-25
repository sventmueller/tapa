//! The self-test's steps for remote workspaces (E25): `remotesheet:` opens the sheet of a new one, `remotefill:` fills
//! it with `host|port|user|password|folder` and connects, or cancels when empty; `askpass:<text>` answers the question
//! of ssh a sheet asks now, `askpass:` cancels it; `remote:<name>` prints each remote workspace with its connection,
//! the stand-ins of its agents and the agents it can start, then the window's heading, `remote:select` selecting it too
//! and `remote:reconnect` connecting it again. `TAPA_REMOTE_SSH_OPTIONS` gives ssh a test's own configuration.

use crate::state::app;

/// Runs `step` if it is one of these; whether it was.
pub(super) fn run(step: &str) -> bool {
    if step == "remotesheet:" {
        if let Some(window) = app().front() {
            window.new_remote_workspace();
        }
    } else if let Some(values) = step.strip_prefix("remotefill:") {
        let values: Vec<&str> = values.split('|').collect();
        let filled = crate::remote::sheet::fill_shown((values.len() > 1).then_some(&values[..]));
        println!("remotefill: sheet {filled}");
    } else if let Some(text) = step.strip_prefix("askpass:") {
        let answered = crate::remote::answer_shown((!text.is_empty()).then_some(text));
        println!("askpass: {answered:?}");
    } else if let Some(name) = step.strip_prefix("remote:") {
        print(name);
    } else {
        return false;
    }
    true
}

fn print(name: &str) {
    let Some(window) = app().front() else { return };
    for workspace in window.workspaces() {
        let Some(remote) = workspace.remote_machine() else {
            continue;
        };
        if name == "select" {
            window.select_workspace(workspace.id);
        }
        if name == "reconnect" {
            remote.connect();
        }
        let row = remote.sidebar();
        let agents: Vec<String> = term_ui_agent::agents(&workspace.base)
            .iter()
            .map(|agent| agent.name().to_string())
            .collect();
        println!(
            "remote {name}: {} | {} | {:?} | stand-ins {:?} | agents {agents:?}",
            workspace.name(),
            row.place(),
            row.connection,
            remote.stand_ins()
        );
    }
    println!("remote {name}: heading {}", window.heading());
}
