//! Writing an editor's document to its file. Every write runs on the disk lane, one after the other, so a volume that
//! stops answering holds the lane and not the window: ⌘S returns at once and the editor takes note when the write is
//! done, and closing and quitting go on from the answer, since what follows depends on it.

use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use splat_i18n::{t, tf};
use term_base::explain::Explain;
use term_document::{ChangedOnDisk, SaveError, SaveJob, Saved};
use term_ui::kit::alert::{confirm, warn};
use term_ui::workspace_host::Tabs;

use super::{EditorTextView, display_name, indent};
use crate::util::{after_frame, main_on_run_loop, ns};

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;

/// Whether a write of the editor's document runs on the disk lane, and whether ⌘S asked for another meanwhile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Writing {
    #[default]
    Idle,
    Running,
    /// Running, and the document saves again once it is done.
    Again,
}

/// What runs on the main thread once a save is over, with whether it wrote the file: not when the user cancelled or
/// the write failed, which the user was told.
pub(crate) type Then = Box<dyn FnOnce(&EditorTextView, bool)>;

/// A save that waits for the write still running, so that the writes of a document keep their order.
pub(super) type Waiting = Box<dyn FnOnce(&EditorTextView)>;

/// Saves `view` on the disk lane, behind a write of it that still runs, and calls `then` once the file is written or
/// the save is over; `save_as` always asks for a file, at once.
pub(crate) fn save_then(view: &EditorTextView, save_as: bool, then: Then) {
    match pick(view, save_as) {
        Some(target) => write_then(view, target, then),
        None => answer(view, then, false),
    }
}

/// Saves `view` as ⌘S does: the write runs on the disk lane, and the editor takes note when it is done.
pub(crate) fn save_soon(view: &EditorTextView, save_as: bool) {
    let writing = &view.ivars().file.writing;
    if writing.get() != Writing::Idle && !save_as {
        writing.set(Writing::Again);
        return;
    }
    save_then(view, save_as, Box::new(|_, _| {}));
}

/// The file to save to: `Some` with a new one the user picked, `None` for the document's own; `None` outside when the
/// user cancelled the pick.
#[expect(clippy::option_option, reason = "cancelled, the document's own file, or a new one")]
fn pick(view: &EditorTextView, save_as: bool) -> Option<Option<PathBuf>> {
    if save_as || view.document().path().is_none() {
        Some(Some(ask_path(view)?))
    } else {
        Some(None)
    }
}

/// Writes `view` to `target`, or to its own file for `None`, once no other write of it runs.
fn write_then(view: &EditorTextView, target: Option<PathBuf>, then: Then) {
    let file = &view.ivars().file;
    if file.writing.get() != Writing::Idle {
        file.waiting.borrow_mut().push(Box::new(move |view| {
            if target.is_none() && !view.document().is_dirty() {
                answer(view, then, true);
            } else {
                write_then(view, target, then);
            }
        }));
        return;
    }
    indent::before_save(view, target.as_deref());
    match begin(view, target.as_deref(), ChangedOnDisk::Refuse) {
        Some(job) => {
            file.after_write.borrow_mut().push(then);
            write_soon(view, job, target);
        }
        None => answer(view, then, false),
    }
}

/// Calls `then` once the run loop is done with what it handles now, so a caller never hears of its save before it
/// returned.
fn answer(view: &EditorTextView, then: Then, written: bool) {
    let view = Retained::from(view);
    after_frame(move || then(&view, written));
}

fn begin(view: &EditorTextView, target: Option<&Path>, changed: ChangedOnDisk) -> Option<SaveJob> {
    let job = match target {
        Some(path) => view.document().begin_save_as(path.to_path_buf()),
        None => view.document().begin_save(changed),
    };
    job.map_err(|error| failed(view, &error)).ok()
}

fn write_soon(view: &EditorTextView, job: SaveJob, target: Option<PathBuf>) {
    let serial = view.ivars().serial;
    let (pending, kept) = (job.clone(), target.clone());
    let queued = term_base::lane::DISK.ask(
        move || job.run(),
        // The run loop, not the main queue: a wait that turns the run loop inside a block of that queue hears it too.
        move |written| main_on_run_loop(move || written_soon(serial, target, written)),
    );
    match queued {
        Ok(()) => view.ivars().file.writing.set(Writing::Running),
        Err(_) => {
            let done = finish(view, kept, Some(pending.run()));
            ended(view, done);
        }
    }
}

/// The write started for the editor `serial` is done; `None` if it panicked.
fn written_soon(
    serial: crate::text_view::EditorSerial,
    target: Option<PathBuf>,
    written: Option<Result<Saved, SaveError>>,
) {
    let Some(view) = super::find_editor(serial) else { return };
    let again = view.ivars().file.writing.replace(Writing::Idle) == Writing::Again;
    let done = finish(&view, target, written);
    ended(&view, done);
    if again && view.document().is_dirty() {
        save_soon(&view, false);
    }
}

/// A write of `view` is over, `done` if it wrote the file: unless it starts again, the saves that waited for it begin
/// and those that started it hear of it.
fn ended(view: &EditorTextView, done: bool) {
    let file = &view.ivars().file;
    if file.writing.get() != Writing::Idle {
        return;
    }
    let thens = std::mem::take(&mut *file.after_write.borrow_mut());
    let waiting = std::mem::take(&mut *file.waiting.borrow_mut());
    for save in waiting {
        save(view);
    }
    for then in thens {
        answer(view, then, done);
    }
}

/// Takes what the write of `view` to `target` brought; whether the file is written, which the write that overwrites a
/// file changed on disk is not yet.
fn finish(view: &EditorTextView, target: Option<PathBuf>, written: Option<Result<Saved, SaveError>>) -> bool {
    match written {
        Some(Ok(done)) => {
            view.document_mut().end_save(done);
            saved(view, target.as_deref());
            true
        }
        Some(Err(SaveError::ChangedOnDisk)) => {
            let name = view.document().path().map(display_name).unwrap_or_default();
            let agreed = confirm(
                view.mtm(),
                &tf("„{}“ wurde inzwischen außerhalb geändert.", &[&name]),
                t("Sichern überschreibt diese Änderungen."),
                t("Überschreiben"),
            );
            if !agreed {
                return false;
            }
            let Some(job) = begin(view, None, ChangedOnDisk::Overwrite) else {
                return false;
            };
            write_soon(view, job, None);
            false
        }
        Some(Err(error)) => {
            failed(view, &error);
            false
        }
        None => {
            warn(
                view.mtm(),
                t("Sichern fehlgeschlagen"),
                t("Das Schreiben der Datei ist abgebrochen."),
            );
            false
        }
    }
}

fn failed(view: &EditorTextView, error: &SaveError) {
    warn(view.mtm(), t("Sichern fehlgeschlagen"), &error.explain());
}

fn saved(view: &EditorTextView, new_path: Option<&Path>) {
    super::update_dirty(view);
    super::recovery::discard(view);
    super::conflict::saved(view);
    if let (Some(path), Some(controller)) = (new_path, view.controller()) {
        if let Some(tab) = view.ivars().tab.get() {
            controller.set_tab_title(tab, &super::display_name(path));
            controller.rekey_tab(tab, Some(crate::tab_key(path)));
        }
        let language = term_document::syntax::Language::detect(Some(path), "");
        view.document_mut().set_language(language);
        super::highlight::recolor_all(view);
        indent::configure(view);
        super::changes::reload(view, true);
        crate::markdown::refresh_bar(view);
    }
    if let (Some(path), Some(controller), Some(host)) = (
        view.document().path().map(Path::to_path_buf),
        view.controller(),
        crate::host::installed(),
    ) {
        host.saved(controller.id(), &path);
    }
}

/// Asks for the file to save to, offering the document's own name and folder, else the project's folder.
fn ask_path(view: &EditorTextView) -> Option<PathBuf> {
    let panel = NSSavePanel::savePanel(view.mtm());
    let own = view.document().path().map(Path::to_path_buf);
    #[cfg(feature = "selftest")]
    let own = take_offered().or(own);
    let folder = own
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| view.controller().and_then(|c| c.project_root()));
    if let Some(folder) = folder {
        panel.setDirectoryURL(Some(&objc2_foundation::NSURL::fileURLWithPath(&ns(
            &folder.to_string_lossy()
        ))));
    }
    if let Some(name) = own.as_deref().and_then(Path::file_name) {
        panel.setNameFieldStringValue(&ns(&name.to_string_lossy()));
    }
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    panel
        .URL()
        .and_then(|url| url.path())
        .map(|p| PathBuf::from(p.to_string()))
}

/// Whether the menu item `item` saves, which an editor always can: `Some(true)` for "Sichern" and "Sichern unter …",
/// `None` for any other item.
pub(crate) fn validate(item: &objc2_app_kit::NSMenuItem) -> Option<bool> {
    let action = item.action()?;
    (action == objc2::sel!(termSave:) || action == objc2::sel!(termSaveAs:)).then_some(true)
}
