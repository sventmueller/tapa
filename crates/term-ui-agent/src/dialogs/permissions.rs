//! `/permissions`: the agent's permission rules grouped as Allow, Ask and Deny, with where each lives, the folders it
//! may work in besides its own, and adding and removing rules through the agent's own writer.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::{t, tf};

use super::backend::Backend;
use super::model::{Behavior, Blocked, PermissionCaps, Permissions, Scope};
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    let caps = backend.permission_caps();
    backend.permissions(Box::new(answer(
        tab.mtm,
        open,
        move |tab, open, result: Result<Permissions, Explained>| match result {
            Ok(permissions) => show(tab, open, &permissions, &caps),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn heading(behavior: Behavior) -> &'static str {
    match behavior {
        Behavior::Allow => t("Erlauben"),
        Behavior::Ask => t("Fragen"),
        Behavior::Deny => t("Verbieten"),
    }
}

pub(super) fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::Local => t("Nur dieses Projekt, nur für mich"),
        Scope::Project => t("Projekt"),
        Scope::User => t("Alle meine Projekte"),
    }
}

fn show(tab: &AgentTab, open: &Open, permissions: &Permissions, caps: &PermissionCaps) {
    let mtm = tab.mtm;
    open.clear_fields();
    let mut views: Vec<Retained<NSView>> = Vec::new();
    for behavior in Behavior::ALL {
        views.push(rows::section(mtm, heading(behavior)));
        let rules: Vec<_> = permissions.rules.iter().filter(|r| r.behavior == behavior).collect();
        if rules.is_empty() {
            views.push(rows::text(mtm, t("Keine Regeln.")));
        }
        for rule in rules {
            let detail = rule
                .wording
                .as_ref()
                .map_or_else(|| rule.place.clone(), |w| format!("{w} · {}", rule.place));
            let text = rows::lines(mtm, rows::BESIDE_ONE, &rule.rule, false, &[&detail]);
            let removed = rule.clone();
            let remove = rows::button(mtm, open, t("Entfernen"), move |tab| {
                let removed = removed.clone();
                super::change(tab, move |backend, done| backend.rule_remove(&removed, done));
            });
            let blocked: Blocked = if rule.editable {
                caps.remove
            } else {
                Some(t("Diese Regel lässt sich hier nicht ändern."))
            };
            rows::block(&remove, blocked);
            views.push(ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &remove])));
        }
    }
    if !permissions.folders.is_empty() {
        views.push(rows::section(mtm, t("Weitere Ordner")));
        for folder in &permissions.folders {
            views.push(rows::pair(mtm, None, folder));
        }
    }
    views.push(rows::section(mtm, t("Regel hinzufügen")));
    let behaviors: Vec<(&str, Blocked)> = Behavior::ALL.iter().map(|b| (heading(*b), None)).collect();
    let scopes: Vec<(&str, Blocked)> = Scope::ALL
        .iter()
        .map(|s| {
            (
                scope_name(*s),
                (!caps.scopes.contains(s)).then(|| t("Dieser Agent kann das hier nicht.")),
            )
        })
        .collect();
    views.push(rows::labelled(
        mtm,
        t("Regel"),
        &rows::field(mtm, open, "rule", "Bash(git push *)"),
    ));
    views.push(rows::labelled(
        mtm,
        t("Wirkung"),
        &rows::choice(mtm, open, "behavior", t("Wirkung"), &behaviors),
    ));
    views.push(rows::labelled(
        mtm,
        t("Gilt für"),
        &rows::choice(mtm, open, "scope", t("Gilt für"), &scopes),
    ));
    open.frame.set_rows(&views);
    let add = rows::button(mtm, open, t("Hinzufügen"), |tab| {
        let Some(open) = tab.dialog.borrow().clone() else {
            return;
        };
        let rule = open.text("rule");
        if rule.is_empty() {
            open.frame.set_notice(Some(t("Die Regel ist leer.")));
            return;
        }
        let behavior = Behavior::ALL[open.chosen("behavior").unwrap_or(0).min(2)];
        let scope = Scope::ALL[open.chosen("scope").unwrap_or(0).min(2)];
        super::change(tab, move |backend, done| backend.rule_add(&rule, behavior, scope, done));
    });
    rows::block(&add, caps.add);
    open.frame.set_buttons(&[ui::view(&add)]);
    open.frame.set_notice(Some(&tf(
        "{} Regeln. Regeln aus der Sitzung oder von der Organisation lassen sich hier nicht ändern.",
        &[&permissions.rules.len().to_string()],
    )));
}
