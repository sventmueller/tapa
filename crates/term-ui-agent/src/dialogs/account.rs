//! `/logout`: who is signed in to the agent, and signing out after the user confirmed it. Signing out ends the
//! running sessions of that agent in every tab, which then ask for signing in again.

use objc2_app_kit::NSView;
use splat_i18n::{t, tf};

use super::backend::Backend;
use super::model::Account;
use super::{Open, answer, rows};
use crate::acp::Agent;
use crate::source::Problem;
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.account(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Account, Explained>| match result {
            Ok(account) => show(tab, open, &account),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, account: &Account) {
    let mtm = tab.mtm;
    open.frame.set_notice(None);
    let said = if account.signed_in {
        t("Angemeldet")
    } else {
        t("Nicht angemeldet")
    };
    let mut views: Vec<objc2::rc::Retained<NSView>> = vec![
        rows::pair(mtm, Some(t("Konto")), said),
        rows::pair(mtm, Some(t("Art")), &account.method),
    ];
    if let Some(name) = &account.name {
        views.push(rows::pair(mtm, Some(t("Adresse")), name));
    }
    open.frame.set_rows(&views);
    let leave = rows::button(mtm, open, t("Abmelden"), |tab| {
        let name = if tab.agent.is_acp() {
            tab.agent.name()
        } else {
            "Claude Code"
        };
        let question = tf("Von {} abmelden? Laufende Sitzungen in allen Tabs enden.", &[name]);
        if !rows::confirm(tab.mtm, &question, t("Abmelden")) {
            return;
        }
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        open.frame.set_notice(Some(t("Wird ausgeführt …")));
        let done = answer(
            tab.mtm,
            &open,
            |tab, open, result: Result<(), Explained>| match result {
                Ok(()) => {
                    signed_out(&tab.agent);
                    tab.refill_dialog();
                }
                Err(why) => open.frame.set_notice(Some(&why.0)),
            },
        );
        super::backend::of(tab).sign_out(Box::new(done));
    });
    rows::block(&leave, (!account.signed_in).then(|| t("Niemand ist angemeldet.")));
    open.frame.set_buttons(&[ui::view(&leave)]);
}

/// Ends the sessions of `agent` in every tab, whose credentials are gone now.
fn signed_out(agent: &Agent) {
    for other in crate::all().into_iter().filter(|other| &other.agent == agent) {
        let source = other.state.borrow_mut().end();
        source.end();
        let problem = if agent.is_acp() {
            Problem::AcpSignIn
        } else {
            Problem::SignIn(t("Du hast dich abgemeldet.").to_string())
        };
        other.start_failed(problem);
    }
}
