//! `/config` and `/output-style`: the agent's settings in effect with where each comes from, the output style to
//! choose, and the settings files to open in an editor. Only what the agent's writer takes can be changed
//! here; the rest is read-only and opens in its file.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use splat_i18n::t;

use super::backend::Backend;
use super::model::Settings;
use super::{Open, answer, rows};
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.settings(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Settings, Explained>| match result {
            Ok(settings) => show(tab, open, &settings),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, settings: &Settings) {
    let mtm = tab.mtm;
    open.clear_fields();
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if let Some((current, styles)) = &settings.styles {
        views.push(rows::section(mtm, t("Ausgabestil")));
        let items: Vec<(&str, super::model::Blocked)> = styles.iter().map(|s| (s.as_str(), None)).collect();
        let choice = rows::choice(mtm, open, "style", t("Ausgabestil"), &items);
        let object: &objc2::runtime::AnyObject = &choice;
        if let (Some(popup), Some(index)) = (
            object.downcast_ref::<objc2_app_kit::NSPopUpButton>(),
            styles.iter().position(|s| s == current),
        ) {
            popup.selectItemAtIndex(isize::try_from(index).unwrap_or(0));
        }
        let styles = styles.clone();
        let apply = rows::button(mtm, open, t("Übernehmen"), move |tab| {
            let Some(open) = tab.dialog.borrow().clone() else {
                return;
            };
            let Some(style) = open.chosen("style").and_then(|i| styles.get(i).cloned()) else {
                return;
            };
            super::change(tab, move |backend, done| backend.set_style(&style, done));
        });
        views.push(ui::view(&ui::row(mtm, 8.0, &[&choice, &ui::spacer(mtm), &apply])));
    }
    views.push(rows::section(mtm, t("Einstellungen")));
    for setting in &settings.settings {
        if setting.choices.is_empty() {
            let column = rows::lines(mtm, rows::VALUE_WIDTH, &setting.value, false, &[&setting.source]);
            views.push(rows::labelled(mtm, &setting.label, &column));
        } else {
            views.push(choice_row(tab, open, setting));
        }
    }
    if !settings.files.is_empty() {
        views.push(rows::section(mtm, t("Einstellungsdateien")));
    }
    for (label, path) in &settings.files {
        let opened = path.clone();
        let button = rows::button(mtm, open, t("Datei öffnen"), move |tab| {
            if let Some(controller) = tab.controller() {
                term_ui::workspace_host::Files::open_file(&controller, &opened);
            }
            tab.close_dialog();
        });
        rows::block(&button, (!path.exists()).then(|| t("Die Datei gibt es noch nicht.")));
        views.push(rows::labelled(
            mtm,
            label,
            &ui::view(&ui::row(mtm, 0.0, &[&button, &ui::spacer(mtm)])),
        ));
    }
    open.frame.set_rows(&views);
}

/// A setting the agent lets the user change: its choices and Apply.
fn choice_row(tab: &AgentTab, open: &Open, setting: &super::model::Setting) -> Retained<NSView> {
    let mtm = tab.mtm;
    let items: Vec<(&str, super::model::Blocked)> =
        setting.choices.iter().map(|(_, name)| (name.as_str(), None)).collect();
    let choice = rows::choice(mtm, open, "setting", &setting.label, &items);
    let object: &objc2::runtime::AnyObject = &choice;
    let popup = object
        .downcast_ref::<objc2_app_kit::NSPopUpButton>()
        .map(objc2::Message::retain);
    if let (Some(popup), Some(index)) = (
        &popup,
        setting.choices.iter().position(|(id, _)| *id == setting.current),
    ) {
        popup.selectItemAtIndex(isize::try_from(index).unwrap_or(0));
    }
    let (key, choices) = (setting.key.clone(), setting.choices.clone());
    let apply = rows::button(mtm, open, t("Übernehmen"), move |tab| {
        let chosen = popup
            .as_ref()
            .and_then(|p| usize::try_from(p.indexOfSelectedItem()).ok());
        let Some((value, _)) = chosen.and_then(|i| choices.get(i)).cloned() else {
            return;
        };
        let key = key.clone();
        super::change(tab, move |backend, done| backend.set_setting(&key, &value, done));
    });
    rows::labelled(mtm, &setting.label, &ui::view(&ui::row(mtm, 8.0, &[&choice, &apply])))
}
