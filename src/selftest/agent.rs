//! The agent steps of the self-test (E21): `agent:<claude|codex|opencode|acp:<id>>` opens a workspace in the
//! self-test's folder with an agent tab that talks to a stand-in (`TAPA_SELFTEST_AGENT=replay:<fixture>` plays a
//! recording of `term_claude` into a Claude tab instead), `agentsay:<text>` sends a prompt, `agentfocus:` moves the
//! keyboard into the composer or out of it as ⌘Esc does, `agentanswer:allow|deny|always` answers the approval that
//! waits, `agenttext:<name>` prints the timeline, `agentdialog:<name>` the texts of the open overlay,
//! `agentdialogsearch:<text>` types into its search field, `agentdialogpress:<title>` presses its button so titled, `agentstatus:<name>` the tab's status with the sidebar's,
//! `agentargs:<name>` the arguments the `claude` stand-in started with, `agentsleep:<name>` parks the agent tab's
//! process as a hidden idle tab does, and `processes:<name>` prints what the supervisor runs.

use std::path::PathBuf;
use std::rc::Rc;

use term_ui::workspace_host::{Workspace as Handle, WorkspaceId};
use term_ui_agent::{AcpAgent, Agent, AgentTab};

use super::stand_in;
use crate::state::app;
use crate::tab::TabContent;

/// Points every agent at a stand-in before the first step, so that no CLI installed on the Mac ever starts.
pub(crate) fn prepare() {
    let project = project();
    let _ = std::fs::create_dir_all(&project);
    let _ = std::fs::write(project.join("hello.txt"), "hello\n");
    match stand_in::claude(&project) {
        Ok(program) => term_ui_agent::set_test_claude(program),
        Err(error) => eprintln!("selftest: the claude stand-in: {error}"),
    }
    let exe = std::env::current_exe().unwrap_or_default();
    term_ui_agent::set_test_codex(Some(term_codex::Launch::new(
        exe.clone(),
        stand_in::env("codex"),
        &project,
    )));
    term_ui_agent::set_test_opencode(Some(term_opencode::Launch::new(
        exe,
        stand_in::env("opencode"),
        &project,
    )));
    term_ui_agent::set_test_launch(None);
}

/// The base path of the agents' workspace.
fn project() -> PathBuf {
    stand_in::dir().with_file_name("agent-project")
}

/// Runs `step` if it is one of the agent steps; whether it was.
pub(crate) fn run(step: &str) -> bool {
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    match name {
        "agent" => open(argument),
        "agentsay" => {
            if let Some(tab) = front_agent() {
                tab.type_and_send(argument);
            }
        }
        "agentfocus" => {
            if let Some(workspace) = app().front().and_then(|window| window.selected_workspace()) {
                term_ui_agent::toggle_focus(&Handle::new(WorkspaceId::new(workspace.id)));
            }
        }
        "agentanswer" => answer(argument),
        "agenttext" => text(argument),
        "agentdialog" => {
            let texts = front_agent().and_then(|tab| tab.dialog_texts());
            println!("agentdialog {argument}: {texts:?}");
        }
        "agentdialogsearch" => {
            let searched = front_agent().is_some_and(|tab| tab.search_dialog(argument));
            println!("agentdialogsearch {argument}: {searched}");
        }
        "agentdialogpress" => {
            let pressed = front_agent().is_some_and(|tab| tab.press_dialog_button(argument));
            println!("agentdialogpress {argument}: {pressed}");
        }
        "agentstatus" => status(argument),
        "agentargs" => println!("agentargs {argument}: {:?}", stand_in::claude_args()),
        "agentsleep" => sleep(argument),
        "processes" => processes(argument),
        _ => return false,
    }
    true
}

/// Opens the agents' workspace in the window in front and an agent tab of `kind` in it.
fn open(kind: &str) {
    let Some(window) = app().front() else {
        return eprintln!("selftest: no window for agent:{kind}");
    };
    window.open_workspace(project());
    let Some(workspace) = window.selected_workspace() else {
        return;
    };
    let handle = Handle::new(WorkspaceId::new(workspace.id));
    if kind == "claude"
        && let Some(fixture) = std::env::var("TAPA_SELFTEST_AGENT")
            .ok()
            .and_then(|value| value.strip_prefix("replay:").map(PathBuf::from))
    {
        let tab = term_ui_agent::open_replay_in(&handle, workspace.base.clone(), "Claude");
        match std::fs::read_to_string(&fixture) {
            Ok(lines) => tab.feed(&lines),
            Err(error) => eprintln!("selftest: {}: {error}", fixture.display()),
        }
        return;
    }
    let agent = match kind {
        "claude" => Agent::Claude,
        "codex" => Agent::Codex,
        "opencode" => Agent::OpenCode,
        other => {
            let Some(id) = other.strip_prefix("acp:") else {
                return eprintln!("selftest: no agent {other}");
            };
            let Some(preset) = AcpAgent::presets().into_iter().find(|preset| preset.id() == id) else {
                let Some(entry) = AcpAgent::find(id) else {
                    return eprintln!("selftest: no ACP preset or registry entry {id}");
                };
                term_ui_agent::set_test_launch(None);
                if term_ui_agent::open_agent(&handle, Agent::Acp(entry), None).is_none() {
                    eprintln!("selftest: agent:{kind} opened no tab");
                }
                return;
            };
            let exe = std::env::current_exe().unwrap_or_default();
            term_ui_agent::set_test_launch(Some(term_acp::Launch::new(
                id,
                id,
                exe,
                Vec::new(),
                stand_in::env("acp:basic"),
                &workspace.base,
            )));
            Agent::Acp(preset)
        }
    };
    if term_ui_agent::open_agent(&handle, agent, None).is_none() {
        eprintln!("selftest: agent:{kind} opened no tab");
    }
}

/// The agent tab opened last in the selected workspace of the window in front.
pub(super) fn front_agent() -> Option<Rc<AgentTab>> {
    let workspace = app().front()?.selected_workspace()?;
    term_ui_agent::of_workspace(&Handle::new(WorkspaceId::new(workspace.id)))
        .into_iter()
        .last()
}

/// Answers the newest approval of the agent tab in front with `how`.
fn answer(how: &str) {
    let Some(tab) = front_agent() else {
        return eprintln!("selftest: no agent tab to answer");
    };
    let what = match how {
        "allow" => term_ui_agent::ALLOW_ONCE,
        "always" => term_ui_agent::ALLOW_ALWAYS,
        "deny" => term_ui_agent::DENY,
        other => return eprintln!("selftest: agentanswer:{other} is not allow, deny or always"),
    };
    let asked = tab.entry_ids(|kind| matches!(kind, term_thread::EntryKind::PermissionRequest(_)));
    match asked.last() {
        Some(id) => tab.permission(*id, what, None),
        None => println!("agentanswer {how}: no approval waits"),
    }
}

/// Prints the cards the timeline of the agent tab in front shows.
fn text(name: &str) {
    let Some(tab) = front_agent() else {
        return eprintln!("selftest: no agent tab for agenttext:{name}");
    };
    let seen = tab.inspect();
    println!("agenttext {name} title {:?}: {:?}", seen.title, seen.visible);
    println!("agenttext {name} cards: {:?}", tab.timeline().visible_summaries());
}

/// Prints the status of the agent tab in front, as its tab and the workspace's row in the sidebar show it.
fn status(name: &str) {
    let Some(window) = app().front() else { return };
    let Some(workspace) = window.selected_workspace() else {
        return;
    };
    let Some(tab) = front_agent() else {
        return println!("agentstatus {name}: no agent tab");
    };
    let shown = window
        .hosted_with_key(workspace.id, &tab.tab_key())
        .map(|hosted| hosted.entry().status);
    println!(
        "agentstatus {name}: agent {:?}, tab {shown:?}, sidebar {:?}, live {}, parked {}, trusted {}, app active {}",
        tab.agent_status(),
        workspace.entry().mark(),
        tab.is_live(),
        tab.is_parked_for_selftest(),
        workspace.is_trusted(),
        objc2_app_kit::NSApplication::sharedApplication(app().mtm).isActive()
    );
}

/// Puts the agent tab in front behind the tab before it and parks its process as the grace period had passed.
fn sleep(name: &str) {
    let Some(window) = app().front() else { return };
    let Some(tab) = front_agent() else {
        return println!("agentsleep {name}: no agent tab");
    };
    if window
        .selected_workspace()
        .and_then(|workspace| window.hosted_with_key(workspace.id, &tab.tab_key()))
        .is_some_and(|hosted| window.selected_tab_id() == Some(hosted.id))
    {
        window.select_relative(-1);
    }
    tab.check_parking_for_selftest();
    println!("agentsleep {name}: asked to park");
}

/// Prints the processes the supervisor runs, by kind and label.
fn processes(name: &str) {
    let running: Vec<String> = term_process::Supervisor::global()
        .running()
        .into_iter()
        .map(|process| format!("{:?} {}", process.kind, process.label))
        .collect();
    println!("processes {name}: {running:?}");
}
