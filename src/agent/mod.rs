//! The agent tabs in the app (E21): Claude, Codex, OpenCode and ACP agents in the base path of a workspace, in the
//! view `term-ui-agent` draws. The app hands that crate its workspaces through `term_ui::workspace_host` and the rest
//! through `term_ui_agent::Host`; the crate adds its tabs to the workspace, runs their sessions under the app's
//! supervisor and reports their status.

mod env;
mod files;
mod host;
mod menu;
mod stored;
mod workspace_host;

pub(crate) use menu::AgentMenu;
pub(crate) use stored::rule_folders;

use std::rc::Rc;

use splat_i18n::{t, tf};
use term_ui::workspace_host::{Workspace as Handle, WorkspaceId};
use term_ui_agent::{Agent, KeptSession};

use crate::state::{app, try_app};
use crate::window::Window;
use crate::workspace::Workspace;

static WORKSPACES: workspace_host::Workspaces = workspace_host::Workspaces;
static HOST: host::AgentHost = host::AgentHost;

/// Hands the UI crates the app's workspaces and the agent tabs their host; once, before the first window opens.
pub(crate) fn install() {
    term_ui::workspace_host::install(&WORKSPACES);
    term_ui_agent::install(&HOST);
    env::start();
    config_changed();
}

/// Has the agent tabs follow the settings.
pub(crate) fn config_changed() {
    let config = app().config();
    term_ui_agent::set_text_size(config.agent.font_size);
    term_ui_agent::set_offer(config.agent.offer.clone());
    crate::window::follow_agent_buttons();
}

/// The window and the workspace whose id is `workspace`, while it is open.
fn find(workspace: WorkspaceId) -> Option<(Rc<Window>, Rc<Workspace>)> {
    let app = try_app()?;
    let windows = app.windows.borrow();
    windows.iter().find_map(|window| {
        let found = window.workspaces().into_iter().find(|own| own.id == workspace.get())?;
        Some((window.clone(), found))
    })
}

fn handle(workspace: u64) -> Handle {
    Handle::new(WorkspaceId::new(workspace))
}

/// The agents a new tab in `base` can run, with whether each is installed: Claude, Codex and OpenCode unless the
/// settings leave them out, greyed where their program is missing, then the ACP agents on the login shell's PATH that
/// the settings do not leave out.
pub(crate) fn offered(base: &std::path::Path) -> Vec<(Agent, bool)> {
    let installed = term_ui_agent::agents(base);
    let mut offered: Vec<(Agent, bool)> = [Agent::Claude, Agent::Codex, Agent::OpenCode]
        .into_iter()
        .filter(Agent::offered)
        .map(|agent| {
            let found = installed.contains(&agent);
            (agent, found)
        })
        .collect();
    for agent in installed.into_iter().filter(Agent::offered) {
        if !offered.iter().any(|(known, _)| *known == agent) {
            offered.push((agent, true));
        }
    }
    offered
}

/// Opens a tab with a new session of `agent` in the base path of the window's selected workspace.
pub(crate) fn open(window: &Window, agent: Agent) {
    let Some(workspace) = window.selected_workspace() else {
        return;
    };
    if window.is_quick() {
        return objc2_app_kit::NSBeep();
    }
    term_ui_agent::open_agent(&handle(workspace.id), agent, None);
}

/// The agent a menu action opens a tab of.
fn agent_of(action: term_config::Action) -> Option<Agent> {
    match action {
        term_config::Action::NewClaudeTab => Some(Agent::Claude),
        term_config::Action::NewCodexTab => Some(Agent::Codex),
        term_config::Action::NewOpenCodeTab => Some(Agent::OpenCode),
        _ => None,
    }
}

/// Whether the program of the agent `action` opens a tab of is installed, for the window's selected workspace.
pub(crate) fn installed(window: &Window, action: term_config::Action) -> bool {
    let (Some(agent), Some(workspace)) = (agent_of(action), window.selected_workspace()) else {
        return false;
    };
    term_ui_agent::agents(&workspace.base).contains(&agent)
}

/// Whether `action` opens a tab of an agent the settings no longer offer, which menus and the palette leave out.
pub(crate) fn switched_off(action: term_config::Action) -> bool {
    agent_of(action).is_some_and(|agent| !agent.offered())
}

/// Opens the tab the menu action `action` stands for.
pub(crate) fn perform(window: &Window, action: term_config::Action) {
    if let Some(agent) = agent_of(action) {
        open(window, agent);
    }
}

/// The tab of workspace `workspace` came to the front: its agent tab shows what it held back.
pub(crate) fn shown(workspace: u64) {
    term_ui_agent::show_pending(&handle(workspace));
}

thread_local! {
    /// The sessions of the agent tabs by their key, as they were when quitting ended them, for the restore.
    static ENDED: std::cell::RefCell<Vec<(String, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The session the agent tab with `key` shows, which the restore keeps.
pub(crate) fn kept(key: &str) -> Option<String> {
    let open = term_ui_agent::all()
        .into_iter()
        .find(|tab| tab.tab_key() == key)
        .and_then(|tab| tab.session_id());
    open.or_else(|| {
        ENDED.with(|ended| {
            ended
                .borrow()
                .iter()
                .find(|(own, _)| own == key)
                .map(|(_, session)| session.clone())
        })
    })
}

/// Opens the agent tab of `session` again in workspace `workspace`; the id of its tab, or none when the session is
/// gone.
pub(crate) fn reopen(workspace: u64, session: &str) -> Option<u64> {
    let kept = KeptSession {
        session: session.to_string(),
    };
    let tab = term_ui_agent::restore(&handle(workspace), &kept)?;
    let (window, _) = find(WorkspaceId::new(workspace))?;
    Some(window.hosted_with_key(workspace, &tab.tab_key())?.id)
}

/// Brings the agent tab `key` of workspace `workspace` forward, as its notification was clicked.
pub(crate) fn bring_forward(workspace: u64, key: &str) {
    let Some((window, _)) = find(WorkspaceId::new(workspace)) else {
        return;
    };
    if let Some(tab) = window.hosted_with_key(workspace, key) {
        window.select(tab.id);
        if let Some(ns_window) = window.front_ns_window() {
            ns_window.makeKeyAndOrderFront(None);
        }
        objc2_app_kit::NSApplication::sharedApplication(app().mtm).activate();
    }
}

/// Asks whether to trust workspace `workspace`, so that its agents load the project's configuration (E21).
fn ask_trust(workspace: WorkspaceId) {
    let Some((window, workspace)) = find(workspace) else {
        return;
    };
    if workspace.is_trusted() {
        return;
    }
    let detail = tf(
        "Agenten in „{}“ laden dann die Hooks, MCP-Server und Einstellungen des Projekts.",
        &[&workspace.name()],
    );
    if crate::quit::confirm(app().mtm, t("Arbeitsbereich vertrauen?"), &detail, t("Vertrauen")) {
        workspace.set_trusted(true);
        window.refresh_hosted();
    }
}

/// Ends every agent's session as the app quits.
pub(crate) fn quit() {
    let sessions: Vec<(String, String)> = term_ui_agent::all()
        .into_iter()
        .filter_map(|tab| Some((tab.tab_key(), tab.session_id()?)))
        .collect();
    ENDED.with(|ended| ended.borrow_mut().extend(sessions));
    term_ui_agent::quit();
    term_ui_agent::wait_for_writes(WRITES_LIMIT);
}

/// How long quitting waits for the threads' index to reach the disk.
const WRITES_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);
