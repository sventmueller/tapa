//! `/status`: what the session runs with, section by section, as the agent reports it.

use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::backend::Backend;
use super::{Open, answer, rows};
use crate::tab::AgentTab;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.status(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Vec<super::model::StatusSection>, term_base::explain::Explained>| match result {
            Ok(sections) => show(tab, open, &sections),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, sections: &[super::model::StatusSection]) {
    let mtm = tab.mtm;
    let mut views: Vec<Retained<NSView>> = Vec::new();
    for section in sections {
        if !section.title.is_empty() {
            views.push(rows::section(mtm, &section.title));
        }
        for (name, value) in &section.rows {
            views.push(rows::pair(mtm, name.as_deref(), value));
        }
    }
    open.frame.set_rows(&views);
}
