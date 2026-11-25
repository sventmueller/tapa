//! `/bug` and `/feedback`: a report to the agent's maker. Before anything leaves the Mac the user sees what goes and
//! to whom, as the agent's backend words it.

use objc2_app_kit::NSTextField;
use splat_i18n::t;

use super::backend::Backend;
use super::model::{Blocked, FeedbackKind};
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    let mtm = tab.mtm;
    open.clear_fields();
    let Some(sends) = backend.feedback_sends() else {
        return rows::failed(tab, open, t("Dieser Agent nimmt keine Berichte an."));
    };
    let kinds: Vec<(&str, Blocked)> = FeedbackKind::ALL.iter().map(|k| (kind_name(*k), None)).collect();
    let description = rows::field(
        mtm,
        open,
        "description",
        t("Was ist passiert, und was hättest du erwartet?"),
    );
    if let Some(field) = (&*description as &objc2::runtime::AnyObject).downcast_ref::<NSTextField>() {
        field.setUsesSingleLineMode(false);
        if let Some(cell) = field.cell() {
            cell.setWraps(true);
        }
        ui::fixed_height(field, 110.0);
    }
    open.frame.set_rows(&[
        rows::labelled(mtm, t("Art"), &rows::choice(mtm, open, "kind", t("Art"), &kinds)),
        rows::labelled(
            mtm,
            t("Titel"),
            &rows::field(mtm, open, "title", t("Kurz, worum es geht")),
        ),
        rows::labelled(mtm, t("Beschreibung"), &description),
        rows::text(mtm, sends),
    ]);
    open.frame.set_notice(None);
    let send = rows::button(mtm, open, t("Senden …"), move |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        let description = open.text("description");
        if description.is_empty() {
            open.frame.set_notice(Some(t("Die Beschreibung ist leer.")));
            return;
        }
        if !rows::confirm_showing(tab.mtm, t("Bericht senden?"), sends, t("Senden")) {
            return;
        }
        let kind = FeedbackKind::ALL[open.chosen("kind").unwrap_or(0).min(1)];
        let title = open.text("title");
        open.frame.set_notice(Some(t("Wird ausgeführt …")));
        let done = answer(
            tab.mtm,
            &open,
            |tab, open, result: Result<String, Explained>| match result {
                Ok(said) => {
                    open.clear_fields();
                    open.frame.set_rows(&[rows::text(tab.mtm, &said)]);
                    open.frame.set_notice(None);
                    open.frame.set_buttons(&[]);
                }
                Err(why) => open.frame.set_notice(Some(&why.0)),
            },
        );
        super::backend::of(tab).send_feedback(kind, &title, &description, Box::new(done));
    });
    open.frame.set_buttons(&[ui::view(&send)]);
}

fn kind_name(kind: FeedbackKind) -> &'static str {
    match kind {
        FeedbackKind::Bug => t("Fehler"),
        FeedbackKind::Idea => t("Idee"),
    }
}
