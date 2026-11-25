//! `/export`: the conversation as text, to copy or to save under the name the agent suggests.

use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSSavePanel, NSView};
use splat_i18n::t;

use super::backend::Backend;
use super::model::Exported;
use super::{Open, answer, rows};
use crate::common::constant;
use crate::common::util::ns;
use crate::tab::AgentTab;

/// How much of the conversation the overlay shows before the rest.
const PREVIEW: usize = 4000;

pub(super) fn fill(tab: &AgentTab, open: &Open, backend: &dyn Backend) {
    if !super::needs_session(tab, open, backend) {
        return;
    }
    open.frame.set_rows(&[rows::waiting(tab.mtm)]);
    backend.export(Box::new(answer(
        tab.mtm,
        open,
        |tab, open, result: Result<Exported, term_base::explain::Explained>| match result {
            Ok(exported) => show(tab, open, exported),
            Err(why) => rows::failed(tab, open, &why.0),
        },
    )));
}

fn show(tab: &AgentTab, open: &Open, exported: Exported) {
    let mtm = tab.mtm;
    let preview: String = exported.text.chars().take(PREVIEW).collect();
    let shortened = preview.len() < exported.text.len();
    let mut views: Vec<Retained<NSView>> = vec![rows::text(
        mtm,
        if preview.is_empty() {
            t("Die Unterhaltung ist noch leer.")
        } else {
            &preview
        },
    )];
    if shortened {
        views.push(rows::text(mtm, t("… gekürzt; Kopieren und Sichern nehmen alles.")));
    }
    open.frame.set_rows(&views);
    let text = exported.text.clone();
    let copy = rows::button(mtm, open, t("Kopieren"), move |_| {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&ns(&text), constant!(NSPasteboardTypeString));
    });
    let save = rows::button(mtm, open, t("Sichern …"), move |tab| save_as(tab, &exported));
    open.frame
        .set_buttons(&[crate::ui::view(&copy), crate::ui::view(&save)]);
}

/// Asks where to save the conversation, then writes it there on the disk lane.
fn save_as(tab: &AgentTab, exported: &Exported) {
    let Some(path) = save_path(tab, &exported.file_name) else {
        return;
    };
    let text = exported.text.clone();
    let controller = tab.controller();
    let _ = term_base::lane::DISK.ask(
        move || std::fs::write(&path, text),
        move |written| {
            if !matches!(written, Some(Ok(()))) {
                crate::common::util::main_soon(move || {
                    if let Some(controller) = controller {
                        term_ui::workspace_host::Notices::show_notice(
                            &controller,
                            t("Die Unterhaltung ließ sich nicht sichern."),
                        );
                    }
                });
            }
        },
    );
}

/// Where the user saves the conversation, offered under `file_name`; none when the save panel is cancelled.
fn save_path(tab: &AgentTab, file_name: &str) -> Option<std::path::PathBuf> {
    #[cfg(feature = "selftest")]
    if std::env::var_os("IDE_SELFTEST").is_some() {
        return Some(selftest_probe::save_path(file_name));
    }
    let panel = NSSavePanel::savePanel(tab.mtm);
    panel.setNameFieldStringValue(&ns(file_name));
    if panel.runModal() != objc2_app_kit::NSModalResponseOK {
        return None;
    }
    panel
        .URL()
        .and_then(|url| url.path())
        .map(|p| std::path::PathBuf::from(p.to_string()))
}

#[cfg(feature = "selftest")]
mod selftest_probe;
