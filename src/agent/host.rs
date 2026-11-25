//! The app as the agent tabs' host beside the workspace (E3, E21): the terminals and files of a workspace, trust,
//! notifications and what the user picked last for each agent.

use std::cell::OnceCell;
use std::path::Path;
use std::rc::Rc;

use term_ui::workspace_host::WorkspaceId;
use term_ui_agent::{AgentChoice, Hit};

use super::find;
use crate::state::app;
use crate::tab::Tab;

/// How many rows of a terminal an agent gets as `@terminal`.
const RECENT_ROWS: usize = 200;

/// The app as the agent tabs' host.
pub(crate) struct AgentHost;

thread_local! {
    static STATE: OnceCell<Rc<term_ui_agent::State>> = const { OnceCell::new() };
}

impl term_ui_agent::Host for AgentHost {
    fn state(&self) -> Rc<term_ui_agent::State> {
        STATE.with(|state| state.get_or_init(Rc::default).clone())
    }

    fn recent_output(&self, workspace: WorkspaceId) -> Option<(String, String)> {
        let (_, workspace) = find(workspace)?;
        let terminals: Vec<Rc<Tab>> = workspace
            .tabs()
            .into_iter()
            .filter(|tab| tab.terminal().is_some())
            .collect();
        let tab = workspace
            .selected_tab()
            .filter(|tab| tab.terminal().is_some())
            .or_else(|| terminals.last().cloned())?;
        let view = tab.focused_view()?;
        let rows = view.with_session(term_session::Session::rows_text)?.rows;
        let mut rows: Vec<String> = rows.into_iter().rev().skip_while(|row| row.trim().is_empty()).collect();
        rows.truncate(RECENT_ROWS);
        rows.reverse();
        Some((tab.title(), rows.join("\n")))
    }

    fn query_files(&self, workspace: WorkspaceId, query: &str, limit: usize, answer: Box<dyn FnOnce(Vec<Hit>) + Send>) {
        let Some((_, workspace)) = find(workspace) else {
            return answer(Vec::new());
        };
        let base = workspace.base.clone();
        let query = query.to_string();
        term_base::thread::detach_or_run("files after @", move || {
            answer(super::files::matching(&base, &query, limit));
        });
    }

    fn open_terminal(&self, workspace: WorkspaceId, cwd: &Path) {
        if let Some((window, workspace)) = find(workspace) {
            window.select_workspace(workspace.id);
            window.new_tab_in(Some(cwd.to_path_buf()));
        }
    }

    fn remember_claude_rules(&self, cwd: &Path) {
        super::stored::remember_rule_folder(cwd.to_path_buf());
    }

    fn turn_ended(&self, workspace: WorkspaceId, _cwd: &Path) {
        if let Some((window, workspace)) = find(workspace) {
            workspace.refresh_branch();
            window.refresh_hosted();
        }
    }

    fn trust(&self, workspace: WorkspaceId) {
        if let Some((window, workspace)) = find(workspace) {
            workspace.set_trusted(true);
            window.refresh_hosted();
        }
    }

    fn post_notice(&self, workspace: WorkspaceId, key: &str, title: &str, body: &str) {
        crate::notify::post_agent(workspace.get(), key, title, body);
    }

    fn agent_choice(&self, agent: &str) -> AgentChoice {
        super::stored::choice(agent, &app().config().agent)
    }

    fn remember_choice(&self, agent: &str, choice: AgentChoice) {
        super::stored::remember(agent, choice, &app().config().agent);
    }
}
