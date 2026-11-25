//! `/resume`: the agent's past sessions of the folder, newest first, each continued in a tab of its own; a tab that
//! holds no conversation yet gives its place to the one continued.

use std::path::Path;
use std::time::SystemTime;

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::t;
use term_thread::{Lane, ThreadIndex};

use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::{Resume, ui};

/// How many past sessions the overlay lists at most.
const LISTED: usize = 200;

/// A past session with when it was last active and the id of the agent that ran it.
type Past = (Resume, SystemTime, String);

pub(super) fn fill(tab: &AgentTab, open: &Open) {
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    let shown = answer(tab.mtm, open, |tab, open, past: Vec<Past>| show(tab, open, past));
    let cwd = tab.cwd.clone();
    let read = move || shown(read_past(&cwd));
    if let Err(why) = crate::common::util::spawn("agent-resume", read) {
        rows::failed(tab, open, &why.0);
    }
}

/// The past sessions of `cwd` with their agents. Reads the disk.
fn read_past(cwd: &Path) -> Vec<Past> {
    let index = ThreadIndex::open(&ThreadIndex::default_dir());
    crate::agent_panel::past_sessions(cwd, LISTED)
        .into_iter()
        .map(|(resume, when)| {
            let agent = crate::agent_panel::agent_id_of(&index, &resume);
            (resume, when, agent)
        })
        .collect()
}

fn show(tab: &AgentTab, open: &Open, past: Vec<Past>) {
    let mtm = tab.mtm;
    let own = own_session(tab);
    let past: Vec<(Resume, SystemTime)> = past
        .into_iter()
        .filter(|(resume, _, agent)| agent == tab.agent.id() && own.as_deref() != Some(resume.session.as_str()))
        .map(|(resume, when, _)| (resume, when))
        .collect();
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if past.is_empty() {
        views.push(rows::text(mtm, t("Keine früheren Sitzungen")));
    }
    for (resume, when) in past {
        let title = resume.title.clone().unwrap_or_else(|| resume.session.clone());
        let text = rows::lines(
            mtm,
            rows::BESIDE_ONE,
            &crate::format::first_line(&title, 200),
            true,
            &[&crate::format::age(when)],
        );
        let button = rows::button(mtm, open, t("Fortsetzen"), move |tab| {
            continue_session(tab, resume.clone())
        });
        views.push(ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &button])));
    }
    open.frame.set_rows(&views);
    open.frame.reveal_search();
}

/// The CLI's id of the session `tab` runs or continues.
fn own_session(tab: &AgentTab) -> Option<String> {
    let state = tab.state.borrow();
    state
        .thread()
        .session()
        .map(str::to_string)
        .or_else(|| state.resume().map(|r| r.session.clone()))
}

/// Continues `resume` in a tab of its own, or brings its open tab to the front; `tab` closes when it holds nothing yet.
fn continue_session(tab: &AgentTab, resume: Resume) {
    let Some(controller) = tab.controller() else { return };
    let untouched = {
        let state = tab.state.borrow();
        state.resume().is_none()
            && state.thread().entries(&Lane::Main).is_empty()
            && state.status() == term_thread::Status::Idle
    };
    tab.close_dialog();
    if crate::open_agent(&controller, tab.agent.clone(), Some(resume)).is_some()
        && untouched
        && let Some(id) = tab.tab_id.get()
    {
        use term_ui::workspace_host::Tabs;
        controller.close_tab(id);
    }
}
