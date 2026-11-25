//! The agent tab: one Claude, Codex, OpenCode or ACP session per tab in the base path of its workspace, with its
//! timeline, composer and status (E21), as the official Claude Code extension shows it, drawn natively. The crate
//! knows a workspace only by its id (`term_ui::workspace_host::Workspace`) and reaches the app's terminals, the files
//! of the base path, trust and notifications through its `Host` (E3).
//!
//! Every tab runs its session in one `term_agents::Registry` the crate keeps, which starts its processes through
//! `term_process::Supervisor::global()` and flushes on the main thread through `main_soon`; the self-test plays
//! recordings there through `term_claude::replay::Replay` instead.
//!
//! # Opening
//!
//! [`open_agent`] opens a tab with a new session of an [`Agent`] in the base path of a workspace, or with the past
//! session a [`Resume`] names; [`agents`] lists what the "new agent" menu offers. [`restore`] opens the tab of a
//! [`KeptSession`] again as a workspace comes back, its conversation parked until the tab comes to the front.
//!
//! # Status
//!
//! Each tab shows its [`AgentStatus`] on its tab and in the workspace's row of the sidebar through
//! `TabDecor::set_tab_status`; [`AgentTab::agent_status`] reads it, [`running`] names the tabs that work, as quitting asks.
//! A turn that ends while the user looks elsewhere posts a notification through `Host::post_notice` and shows
//! `Attention` until the tab comes to the front.

mod acp;
mod acp_install;
mod acp_words;
mod actions;
mod activity;
mod agent_panel;
mod attrs;
mod cards;
mod choices;
mod codex;
pub mod common;
mod compose;
mod composer;
mod context;
mod context_view;
mod dialogs;
mod driven;
mod enlarge;
mod figures;
mod format;
mod header;
mod host;
mod markdown;
mod mention;
mod model;
mod opencode;
mod output_tab;
mod outside;
mod overlay;
mod refresh;
mod registry;
mod requests;
mod row;
mod source;
mod status;
mod stop;
pub mod style;
mod suggest;
mod tab;
mod tab_driven;
mod tab_parking;
#[cfg(feature = "selftest")]
mod tab_probe;
mod tab_session;
mod target;
mod task_output;
mod text;
mod text_size;
mod timeline;
#[cfg(feature = "selftest")]
mod timeline_probe;
mod ui;
mod usage;
mod waiting;
pub mod work;

use std::cell::RefCell;
use std::rc::Rc;

use term_ui::workspace_host::{AsWorkspace, Notices, Workspace};

pub use acp::{AcpAgent, Agent, agent_name, set_offer};
pub use acp_install::{
    Availability, availability, cached_register, confirm_install, fetch_register, install_plan, installs,
    progress_text, refusal_text,
};
pub use agent_panel::{is_bookmarked, past_session, wait_for_writes};
pub use cards::request::scope_name;
pub use dialogs::remove_claude_rule;
pub use format::{age, elapsed, tokens};
pub(crate) use format::{iso_time, time_of_day};
pub use host::{AgentChoice, Hit, Host, host, install, state};
pub use source::Resume;
pub use stop::Stop;
pub use tab::AgentTab;
pub use term_ui::workspace_host::AgentStatus;
pub use text_size::{CHAT_FONT_MAX, CHAT_FONT_MIN, CHAT_FONT_SIZE, current as text_size, set_text_size};
pub use ui::{framed, icon, insets, separator};

use common::notify;
pub(crate) use driven::Sessions;

/// An agent tab as the restored state keeps it, opened again by resuming its session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeptSession {
    /// The agent's id for the session.
    pub session: String,
}

/// Every open agent tab, in the order they were opened; a tab leaves when it is closed.
#[derive(Default)]
pub(crate) struct AgentTabs {
    tabs: RefCell<Vec<Rc<AgentTab>>>,
    keys: common::registry::Serial,
}

/// The number of an agent tab; never reused while the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentKey(u64);

impl AgentKey {
    /// The number under which the agent tab is known, for a tag or a name.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The state of the agent tabs in the running app, which the app keeps and hands out through `Host::state`.
#[derive(Default)]
pub struct State {
    activity: activity::Activities,
    agent_panels: agent_panel::AgentPanels,
    agent_tabs: AgentTabs,
    enlarged_picture: enlarge::EnlargedPicture,
    figures: figures::Figures,
    context_gauges: context_view::ContextGauges,
    usage: usage::Usage,
    /// The registry the agent tabs run their sessions in, made as the first tab opens.
    sessions: RefCell<Option<Rc<registry::Local>>>,
}

impl State {
    /// The key of an agent tab that opens now, which no tab had before, and the registry it starts its session in.
    pub(crate) fn admit(&self) -> (AgentKey, Option<Rc<dyn Sessions>>) {
        let sessions = self
            .sessions
            .borrow_mut()
            .get_or_insert_with(registry::Local::new)
            .clone();
        (AgentKey(self.agent_tabs.keys.next()), Some(sessions))
    }

    /// The registry, once a tab made it.
    pub(crate) fn registry(&self) -> Option<Rc<registry::Local>> {
        self.sessions.borrow().clone()
    }
}

fn with_tabs<R>(f: impl FnOnce(&RefCell<Vec<Rc<AgentTab>>>) -> R) -> R {
    f(&crate::host::state().agent_tabs.tabs)
}

/// Opens an agent tab with a new session of `agent` in the workspace's base path, or with its past session `resume`;
/// a tab that shows `resume` already comes to the front instead.
pub fn open_agent(
    controller: &(impl AsWorkspace + ?Sized),
    agent: Agent,
    resume: Option<Resume>,
) -> Option<Rc<AgentTab>> {
    let controller = &controller.as_workspace();
    if let Some(resume) = &resume
        && let Some(open) = find_session(controller, &resume.session)
    {
        open.select();
        return Some(open);
    }
    open_new(controller, agent, resume, AgentTab::start)
}

/// Opens the agent tab of `agent`'s past session `resume` again as the workspace comes back: a Claude session shows
/// its conversation parked, its CLI starting once the tab comes to the front or gets a prompt.
pub fn restore_agent(controller: &(impl AsWorkspace + ?Sized), agent: Agent, resume: Resume) -> Option<Rc<AgentTab>> {
    open_new(&controller.as_workspace(), agent, Some(resume), AgentTab::start_parked)
}

/// Opens the tab `kept` names again in the workspace, with the agent its thread names; `None` when the session is
/// no longer in the base path's past sessions. Reads the disk.
pub fn restore(controller: &(impl AsWorkspace + ?Sized), kept: &KeptSession) -> Option<Rc<AgentTab>> {
    let controller = controller.as_workspace();
    let root = controller.project_root()?;
    let (agent, resume) = past_session(&root, &kept.session)?;
    restore_agent(&controller, agent, resume)
}

fn open_new(
    controller: &Workspace,
    agent: Agent,
    resume: Option<Resume>,
    start: fn(&AgentTab),
) -> Option<Rc<AgentTab>> {
    let state = crate::host::state();
    let tab = AgentTab::open(controller, &state, agent, resume)?;
    state.agent_tabs.tabs.borrow_mut().push(tab.clone());
    start(&tab);
    changed(controller);
    Some(tab)
}

/// The agents a new tab in the workspace at `base` can run, in the order the menu lists them: Claude, Codex and
/// OpenCode where their programs are on the login shell's PATH outside `base` or installed by the app, then the ACP
/// agents found there, then those the app installed from the registry (E16, E21); each only while the settings offer
/// it.
pub fn agents(base: &std::path::Path) -> Vec<Agent> {
    let env = &term_ui::workspace_host::environment_for(base, std::time::Duration::ZERO)[..];
    let mut agents: Vec<Agent> = [Agent::Claude, Agent::Codex, Agent::OpenCode]
        .into_iter()
        .filter(|agent| agent.offered() && agent.installed(env, base))
        .collect();
    agents.extend(
        AcpAgent::presets()
            .into_iter()
            .map(Agent::Acp)
            .filter(|agent| agent.offered() && agent.installed(env, base)),
    );
    // The agents the app installed live on this Mac; a remote workspace has only those on the other machine (E25).
    if term_remote::registry::bin_for(base).is_none() {
        agents.extend(
            AcpAgent::installed_entries()
                .into_iter()
                .map(Agent::Acp)
                .filter(|agent| agent.offered() && agent.installed(env, base)),
        );
    }
    agents
}

/// Has the agent tabs working in `base` whose session ended start it again, continuing their conversation, once they
/// are in front or get a prompt, as when a remote workspace's connection came back (E25); parked tabs stay parked.
pub fn resume_in(base: &std::path::Path) {
    for tab in all() {
        let ended = matches!(tab.state.borrow().source(), crate::source::Source::Idle);
        if ended && tab.cwd.starts_with(base) && tab.parking.process.get().is_none() {
            tab.parking.deferred.set(true);
            tab.wake_deferred();
        }
    }
}

/// Claude's own program on the login shell's PATH in `env` or where its installer puts it (E21).
pub fn claude_program(env: &[(String, String)]) -> Option<std::path::PathBuf> {
    source::find_claude(env)
}

/// The tab with `key`, if it is open.
fn get(key: AgentKey) -> Option<Rc<AgentTab>> {
    with_tabs(|tabs| tabs.borrow().iter().find(|t| t.key == key).cloned())
}

/// Every open agent tab.
pub fn all() -> Vec<Rc<AgentTab>> {
    with_tabs(|tabs| tabs.borrow().clone())
}

/// The agent tabs of the workspace.
pub fn of_workspace(controller: &(impl AsWorkspace + ?Sized)) -> Vec<Rc<AgentTab>> {
    let controller = &controller.as_workspace();
    all().into_iter().filter(|t| t.belongs_to(controller)).collect()
}

/// The open tab of the workspace that shows the agent session `session`.
pub(crate) fn find_session(controller: &(impl AsWorkspace + ?Sized), session: &str) -> Option<Rc<AgentTab>> {
    of_workspace(controller)
        .into_iter()
        .find(|t| t.session_id().as_deref() == Some(session))
}

/// Forgets the tab with `key` and ends its session.
fn remove(key: AgentKey) {
    let removed = with_tabs(|tabs| {
        let mut tabs = tabs.borrow_mut();
        let position = tabs.iter().position(|t| t.key == key)?;
        Some(tabs.remove(position))
    });
    if let Some(tab) = removed {
        tab.close();
        if let Some(controller) = tab.controller() {
            changed(&controller);
        }
        common::util::give_back_memory_soon();
    }
}

/// The agent tabs of the workspace that work now, by title: a turn, a subagent or background work that runs.
pub fn running_in(controller: &(impl AsWorkspace + ?Sized)) -> Vec<String> {
    of_workspace(controller)
        .into_iter()
        .filter(|tab| works(tab))
        .map(|tab| tab.title())
        .collect()
}

/// The agent tabs of every workspace that work now, by title, as quitting asks.
pub fn running() -> Vec<String> {
    all()
        .into_iter()
        .filter(|tab| works(tab))
        .map(|tab| tab.title())
        .collect()
}

fn works(tab: &AgentTab) -> bool {
    let thread = tab.view_snapshot();
    thread.status() != term_thread::Status::Idle || tab::delegates(&thread)
}

/// Ends the sessions of a workspace that closes.
pub fn workspace_closed(controller: &(impl AsWorkspace + ?Sized)) {
    for tab in of_workspace(controller) {
        remove(tab.key);
    }
}

/// Ends every session as the app quits; their processes get the supervisor's grace.
pub fn quit() {
    for tab in all() {
        remove(tab.key);
    }
    if let Some(registry) = crate::host::state().registry() {
        registry.stop_all();
    }
}

/// Tells the sidebar of the workspace that sessions changed.
fn changed(controller: &Workspace) {
    controller.refresh_activity();
}

/// Catches up the tab in front after the app switched workspaces or tabs.
pub fn show_pending(controller: &(impl AsWorkspace + ?Sized)) {
    for tab in of_workspace(controller).iter().filter(|tab| tab.is_shown()) {
        tab.show_pending();
    }
}

/// Moves the keyboard focus into the composer of the agent tab in front, or back out of it (⌘Esc); opens a Claude
/// tab when the workspace has none.
pub fn toggle_focus(controller: &(impl AsWorkspace + ?Sized)) {
    let controller = &controller.as_workspace();
    let tabs = of_workspace(controller);
    if let Some(focused) = tabs.iter().find(|t| t.composer_has_focus()) {
        focused.leave_focus();
        return;
    }
    let shown = tabs.iter().find(|t| t.is_shown()).or_else(|| tabs.last()).cloned();
    match shown {
        Some(tab) => {
            tab.select();
            tab.focus_composer();
        }
        None => {
            if let Some(tab) = open_agent(controller, Agent::Claude, None) {
                tab.focus_composer();
            }
        }
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;
