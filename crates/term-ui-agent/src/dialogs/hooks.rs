//! `/hooks`: the agent's hooks by event, with what each runs and where it lives, added, changed and removed through
//! the agent's own writer. Hooks of plugins and of the organisation are shown but stay as they are.

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView};
use splat_i18n::{t, tf};

use super::backend::Backend;
use super::model::{Blocked, Hook, HookDraft, HookEvent, Hooks, Scope};
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    let blocked = backend.hooks_blocked();
    backend.hooks(Box::new(answer(
        tab.mtm,
        open,
        move |tab, open, result: Result<Hooks, Explained>| match result {
            Ok(hooks) => show(tab, open, &hooks, blocked),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, hooks: &Hooks, blocked: Blocked) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame.set_notice(None);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if hooks.hooks.is_empty() {
        views.push(rows::text(mtm, t("Keine Hooks eingerichtet.")));
    }
    let mut last_event = "";
    for hook in &hooks.hooks {
        if hook.event != last_event {
            // Event names are identifiers in camel case, which the section headings' capitals would garble.
            views.push(ui::view(&ui::label(
                mtm,
                &hook.event,
                &crate::style::bold(12.0),
                &NSColor::secondaryLabelColor(),
            )));
            last_event = &hook.event;
        }
        views.push(hook_row(tab, open, hook, &hooks.events, blocked));
    }
    open.frame.set_rows(&views);
    let events = hooks.events.clone();
    let add = rows::button(mtm, open, t("Hinzufügen …"), move |tab| {
        if let Some(open) = tab.dialog.borrow().clone() {
            show_form(tab, &open, &events, None);
        }
    });
    rows::block(&add, blocked);
    open.frame.set_buttons(&[ui::view(&add)]);
}

fn hook_row(tab: &AgentTab, open: &Open, hook: &Hook, events: &[HookEvent], blocked: Blocked) -> Retained<NSView> {
    let mtm = tab.mtm;
    let head = if hook.matcher.is_empty() {
        hook.kind.clone()
    } else {
        format!("{} · {}", hook.matcher, hook.kind)
    };
    let text = rows::lines(mtm, rows::BESIDE_TWO, &head, true, &[&hook.text, &hook.place]);
    let fixed: Blocked = hook
        .definition
        .is_none()
        .then(|| t("Dieser Hook lässt sich hier nicht ändern."))
        .or(blocked);
    let (edited, catalog) = (hook.clone(), events.to_vec());
    let edit = rows::button(mtm, open, t("Bearbeiten …"), move |tab| {
        if let Some(open) = tab.dialog.borrow().clone() {
            show_form(tab, &open, &catalog, Some(&edited));
        }
    });
    rows::block(&edit, fixed);
    let removed = hook.clone();
    let remove = rows::button(mtm, open, t("Entfernen"), move |tab| {
        if rows::confirm(tab.mtm, &tf("Hook „{}“ entfernen?", &[&removed.text]), t("Entfernen")) {
            let removed = removed.clone();
            super::change(tab, move |backend, done| backend.hook_remove(&removed, done));
        }
    });
    rows::block(&remove, fixed);
    ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &edit, &remove]))
}

/// The form for a new hook, or for changing `old`.
fn show_form(tab: &AgentTab, open: &Open, events: &[HookEvent], old: Option<&Hook>) {
    let mtm = tab.mtm;
    open.clear_fields();
    open.frame
        .set_notice(Some(t("Ein Hook führt den Befehl mit den Rechten des Agenten aus.")));
    let names: Vec<(&str, Blocked)> = events.iter().map(|e| (e.name.as_str(), None)).collect();
    let scopes: Vec<(&str, Blocked)> = Scope::ALL
        .iter()
        .map(|s| (super::permissions::scope_name(*s), None))
        .collect();
    let event = rows::choice(mtm, open, "event", t("Ereignis"), &names);
    if let Some(old) = old {
        let object: &objc2::runtime::AnyObject = &event;
        if let (Some(popup), Some(index)) = (
            object.downcast_ref::<objc2_app_kit::NSPopUpButton>(),
            events.iter().position(|e| e.name == old.event),
        ) {
            popup.selectItemAtIndex(isize::try_from(index).unwrap_or(0));
        }
    }
    let matcher = rows::field(mtm, open, "matcher", "Bash");
    let command = rows::field(mtm, open, "command", "echo check");
    let timeout = rows::field(mtm, open, "timeout", "60");
    if let Some(old) = old {
        set_text(&matcher, &old.matcher);
        set_text(&command, &old.text);
        if let Some(seconds) = old.definition.as_ref().and_then(|d| d["timeout"].as_u64()) {
            set_text(&timeout, &seconds.to_string());
        }
    }
    let mut views = vec![
        rows::labelled(mtm, t("Ereignis"), &event),
        rows::labelled(mtm, t("Auswahl"), &matcher),
        rows::labelled(mtm, t("Befehl"), &command),
        rows::labelled(mtm, t("Zeitlimit (s)"), &timeout),
    ];
    if old.is_none() {
        views.push(rows::labelled(
            mtm,
            t("Gilt für"),
            &rows::choice(mtm, open, "scope", t("Gilt für"), &scopes),
        ));
    }
    open.frame.set_rows(&views);
    let catalog = events.to_vec();
    let old = old.cloned();
    let save = rows::button(
        mtm,
        open,
        if old.is_some() { t("Sichern") } else { t("Hinzufügen") },
        move |tab| {
            let Some(open) = tab.dialog.borrow().clone() else {
                return;
            };
            let Some(draft) = draft(&open, &catalog) else {
                open.frame.set_notice(Some(t("Der Hook braucht einen Befehl.")));
                return;
            };
            match old.clone() {
                Some(old) => super::change(tab, move |backend, done| backend.hook_replace(&old, draft, done)),
                None => super::change(tab, move |backend, done| backend.hook_add(draft, done)),
            }
        },
    );
    let cancel = rows::button(mtm, open, t("Abbrechen"), |tab| tab.refill_dialog());
    open.frame.set_buttons(&[ui::view(&cancel), ui::view(&save)]);
}

fn set_text(view: &NSView, text: &str) {
    let object: &objc2::runtime::AnyObject = view;
    if let Some(field) = object.downcast_ref::<objc2_app_kit::NSTextField>() {
        field.setStringValue(&crate::common::util::ns(text));
    }
}

/// The form's hook; none without a command.
fn draft(open: &Open, events: &[HookEvent]) -> Option<HookDraft> {
    let command = open.text("command");
    if command.is_empty() {
        return None;
    }
    let event = events.get(open.chosen("event").unwrap_or(0))?;
    Some(HookDraft {
        event: event.name.clone(),
        matcher: if event.matcher {
            open.text("matcher")
        } else {
            String::new()
        },
        command,
        timeout: open.text("timeout").parse().ok(),
        scope: Scope::ALL[open.chosen("scope").unwrap_or(0).min(2)],
    })
}
