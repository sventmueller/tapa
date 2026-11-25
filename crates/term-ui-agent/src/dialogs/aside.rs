//! `/btw`: a question beside the conversation, answered in the overlay without joining the conversation. The tab
//! keeps the questions and answers, which the next question builds on.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::t;

use super::backend::Backend;
use super::model::Aside;
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

/// Shows the questions so far and asks `question` when there is one.
pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend, question: &str) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    if question.is_empty() {
        show(tab, open, None);
    } else {
        ask(tab, open, backend, question);
    }
}

fn ask(tab: &AgentTab, open: &Open, backend: &dyn Backend, question: &str) {
    show(tab, open, Some(question));
    let before = tab.asides.borrow().clone();
    backend.aside(
        question,
        &before,
        Box::new(answer(
            tab.mtm,
            open,
            |tab, open, result: Result<Aside, Explained>| match result {
                Ok(aside) => {
                    tab.asides.borrow_mut().push(aside);
                    show(tab, open, None);
                }
                Err(why) => {
                    show(tab, open, None);
                    open.frame.set_notice(Some(&why.0));
                }
            },
        )),
    );
}

/// The exchanges so far, `asking` while its answer is out, and the field for the next question.
fn show(tab: &AgentTab, open: &Open, asking: Option<&str>) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame.set_notice(None);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    let asides = tab.asides.borrow();
    if asides.is_empty() && asking.is_none() {
        views.push(rows::text(
            mtm,
            t("Eine Frage nebenbei: Der Agent antwortet hier, und weder Frage noch Antwort kommen in die Unterhaltung."),
        ));
    }
    // Questions are the user's own words, which the section headings' capitals would change.
    for aside in asides.iter() {
        views.push(rows::lines(mtm, rows::TEXT_WIDTH, &aside.question, true, &[]));
        let note = aside.note.as_deref().unwrap_or_default();
        views.push(rows::lines(mtm, rows::TEXT_WIDTH, &aside.answer, false, &[note]));
    }
    drop(asides);
    if let Some(question) = asking {
        views.push(rows::lines(mtm, rows::TEXT_WIDTH, question, true, &[]));
        views.push(rows::waiting(mtm));
    }
    let field = rows::field(mtm, open, "question", t("Was möchtest du wissen?"));
    views.push(rows::labelled(mtm, t("Frage"), &field));
    open.frame.set_rows(&views);
    if let Some(window) = field.window() {
        window.makeFirstResponder(Some(&field));
    }
    let send = rows::button(mtm, open, t("Senden"), |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        let question = open.text("question");
        if question.is_empty() {
            return;
        }
        let backend = super::backend::of(tab);
        ask(tab, &open, &*backend, &question);
    });
    send.setKeyEquivalent(&crate::common::util::ns("\r"));
    rows::block(&send, asking.map(|_| t("Der Agent beantwortet noch die letzte Frage.")));
    open.frame.set_buttons(&[ui::view(&send)]);
}
