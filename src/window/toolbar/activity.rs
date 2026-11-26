//! Terminal commands and supervised processes shown by the toolbar activity popover.

use std::path::Path;
use std::time::SystemTime;

use objc2_app_kit::NSToolbarItem;
use splat_i18n::{t, tf};
use term_process::{Kind, Supervisor};
use term_session::Foreground;
use term_ui_agent::ActivityProcess;

use crate::switcher;
use crate::tab::Tab;

pub(super) fn toggle(item: &NSToolbarItem) {
    term_ui_agent::toggle_toolbar_activity(item, processes, switcher::bring_forward);
}

fn processes() -> Vec<ActivityProcess> {
    let mut rows = Vec::new();
    let home = term_base::home();
    for window in switcher::windows_front_first() {
        for workspace in window.workspaces() {
            for (index, tab) in workspace.tabs().iter().enumerate() {
                append_commands(&mut rows, tab, index + 1, &home);
            }
        }
    }
    let now = SystemTime::now();
    for process in Supervisor::global().running() {
        let (kind, symbol) = match process.kind {
            Kind::LanguageServer => (t("Sprachserver"), "curlybraces"),
            Kind::DevServer => (t("Dev-Server"), "server.rack"),
            Kind::DebugAdapter => (t("Debug-Adapter"), "ladybug"),
            Kind::Test => (t("Test"), "gearshape"),
            Kind::Tool => (t("Werkzeug"), "gearshape"),
            Kind::AgentCommand => (t("Agent-Befehl"), "gearshape"),
            Kind::Gate => (t("Prüfstation"), "gearshape"),
            // Agent sessions and running terminal commands have their own entries. Git helpers are transient.
            Kind::Agent | Kind::Terminal | Kind::Git => continue,
        };
        let elapsed = duration(now.duration_since(process.started).unwrap_or_default().as_secs());
        rows.push(ActivityProcess {
            pane: None,
            managed: Some(process.id),
            symbol,
            title: process.label,
            detail: tf("{} · {} · PID {}", &[kind, &elapsed, &process.pid.to_string()]),
        });
    }
    rows
}

fn duration(seconds: u64) -> String {
    match seconds {
        0..60 => tf("{} s", &[&seconds.to_string()]),
        60..3600 => tf("{} min", &[&(seconds / 60).to_string()]),
        _ => tf("{} h", &[&(seconds / 3600).to_string()]),
    }
}

fn append_commands(rows: &mut Vec<ActivityProcess>, tab: &Tab, number: usize, home: &Path) {
    let Some(terminal) = tab.terminal() else { return };
    let panes = terminal.panes.sessions();
    let sessions = terminal.sessions.borrow();
    for (index, pane) in panes.iter().copied().enumerate() {
        let Some(session) = sessions.get(&pane) else { continue };
        if session.status == term_ui_sidebar::Status::Ended {
            continue;
        }
        let title = if let Some((line, _)) = session.running.as_ref().filter(|(line, _)| !line.is_empty()) {
            line.clone()
        } else {
            match switcher::foreground(pane) {
                Foreground::Shell(_) | Foreground::Unknown => continue,
                front => switcher::program(front),
            }
        };
        let position = if panes.len() > 1 {
            format!("{number}.{}", index + 1)
        } else {
            number.to_string()
        };
        let position = tf("Tab {}", &[&position]);
        let mut detail = format!("{position} · {}", session.short_folder(home));
        if let Some((_, start)) = session.running {
            detail = format!("{} · {detail}", duration(start.elapsed().as_secs()));
        }
        rows.push(ActivityProcess {
            pane: Some(pane),
            managed: None,
            symbol: "terminal",
            title,
            detail,
        });
    }
}
