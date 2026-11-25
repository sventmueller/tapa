//! `/memory`: the instruction files the agent reads, opened or made in an editor tab, the folders it keeps memory in,
//! and automatic memory switched on and off.

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSWorkspace};
use objc2_foundation::NSURL;
use splat_i18n::t;

use super::backend::Backend;
use super::model::Memory;
use super::{Open, answer, rows};
use crate::common::util::ns;
use crate::tab::AgentTab;
use crate::ui;
use term_base::explain::Explained;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.memory(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Memory, Explained>| match result {
            Ok(memory) => show(tab, open, &memory),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, memory: &Memory) {
    let mtm = tab.mtm;
    let mut views: Vec<Retained<NSView>> = vec![rows::section(mtm, t("Anweisungen"))];
    for file in &memory.files {
        let text = rows::lines(mtm, rows::BESIDE_ONE, &file.label, false, &[&file.place]);
        let path = file.path.clone();
        let exists = file.exists;
        let button = rows::button(mtm, open, if exists { t("Öffnen") } else { t("Anlegen") }, move |tab| {
            open_or_make(tab, &path, exists);
        });
        views.push(ui::view(&ui::row(mtm, 8.0, &[&text, &ui::spacer(mtm), &button])));
    }
    if let Some((on, toggleable)) = memory.automatic {
        views.push(rows::section(mtm, t("Automatisches Gedächtnis")));
        let blocked = (!toggleable).then(|| t("Das lässt sich hier nicht umschalten."));
        let switch = rows::switch(mtm, open, on, t("Automatisches Gedächtnis"), blocked, |tab, on| {
            super::change(tab, move |backend, done| backend.set_auto_memory(on, done));
        });
        let said = rows::text(mtm, t("Der Agent merkt sich selbst, was er über das Projekt lernt."));
        views.push(ui::view(&ui::row(mtm, 8.0, &[&said, &ui::spacer(mtm), &switch])));
    }
    for (label, folder) in &memory.folders {
        let folder = folder.clone();
        let button = rows::button(mtm, open, label, move |_| {
            NSWorkspace::sharedWorkspace().openURL(&NSURL::fileURLWithPath(&ns(&folder.to_string_lossy())));
        });
        views.push(ui::view(&button));
    }
    open.frame.set_rows(&views);
}

/// Opens `path` in an editor tab of the tab's window, making it empty first when it does not exist yet.
fn open_or_make(tab: &AgentTab, path: &Path, exists: bool) {
    let Some(controller) = tab.controller() else { return };
    let path: PathBuf = path.to_path_buf();
    if exists {
        term_ui::workspace_host::Files::open_file(&controller, &path);
        tab.close_dialog();
        return;
    }
    let opened = path.clone();
    let _ = term_base::lane::DISK.ask(
        move || {
            if let Some(folder) = path.parent() {
                std::fs::create_dir_all(folder)?;
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map(|_| ())
        },
        move |made| {
            if matches!(made, Some(Ok(()))) {
                crate::common::util::main_soon(move || term_ui::workspace_host::Files::open_file(&controller, &opened));
            }
        },
    );
    tab.close_dialog();
}
