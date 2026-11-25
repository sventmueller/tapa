//! `/skills`: the skills the agent offers, with where each comes from and what its listing costs, switched on and off
//!.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::{t, tf};

use super::backend::Backend;
use super::model::Skill;
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.skills(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Vec<Skill>, Explained>| match result {
            Ok(skills) => show(tab, open, &skills),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, skills: &[Skill]) {
    let mtm = tab.mtm;
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if skills.is_empty() {
        views.push(rows::text(mtm, t("Keine Skills.")));
    }
    for skill in skills {
        let mut detail = skill.source.clone();
        if let Some(tokens) = skill.tokens {
            detail = format!("{detail} · {}", tf("etwa {} Tokens", &[&tokens.to_string()]));
        }
        let text = rows::lines(
            mtm,
            rows::BESIDE_ONE,
            &skill.shown,
            true,
            &[&detail, &skill.description],
        );
        let switched = skill.clone();
        let switch = rows::switch(
            mtm,
            open,
            skill.enabled,
            &tf("{} eingeschaltet", &[&skill.shown]),
            None,
            move |tab, on| {
                let switched = switched.clone();
                super::change(tab, move |backend, done| backend.set_skill(&switched, on, done));
            },
        );
        views.push(ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &switch])));
    }
    open.frame.set_rows(&views);
}
