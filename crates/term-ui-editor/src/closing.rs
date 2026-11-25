//! Closing editors and quitting: the questions about unsaved text, and saving or giving it up. A save the user asks
//! for writes on the disk lane, so the close answers `Later` and goes on once the file is written.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSAlertFirstButtonReturn, NSAlertSecondButtonReturn};
use splat_i18n::{t, tf};
use term_ui::kit::alert::{Alert, Role};

use super::{EditorTextView, conflict, display_name, editor_of_tab, editors_in, open_editors, recovery, saving};
use term_ui::workspace_host::{TabId, Workspace};

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;

/// Whether something may close: now, not at all, or once the saves the user asked for are over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// It may close now.
    Yes,
    /// It stays open.
    No,
    /// It may close once the saves the user asked for are over.
    Later,
}

/// What a close that answered `Later` does once the saves are over, told whether every file was written.
pub type Again = Rc<dyn Fn(bool)>;

/// Writes the copy of every editor of `controller` that holds unsaved text, as after a close the user cancelled.
pub(crate) fn store_copies(controller: &Workspace) {
    for view in editors_in(controller) {
        view.ivars().file.given_up.set(false);
        recovery::store(&view);
    }
}

/// Whether the editor in tab `id` of `controller` may close: saved, or the user said so; `Later` when the user asked
/// to save, and `again` hears once the write is over.
pub(crate) fn may_close(controller: &Workspace, id: TabId, again: &Again) -> Answer {
    let Some(view) = editor_of_tab(controller, id) else {
        return Answer::Yes;
    };
    if view.ivars().file.given_up.get() {
        return Answer::Yes;
    }
    if term_base::unattended::is() {
        give_up(&view);
        return Answer::Yes;
    }
    if conflict::is_gone(&view) {
        return conflict::may_close(&view, again);
    }
    if !view.document().is_dirty() {
        recovery::discard(&view);
        return Answer::Yes;
    }
    let name = view
        .document()
        .path()
        .map_or_else(|| t("Ohne Titel").to_string(), display_name);
    let response = Alert::new(controller.mtm(), &tf("Änderungen an „{}“ sichern?", &[&name]))
        .informative(t("Ohne Sichern gehen deine Änderungen verloren."))
        .button(t("Sichern"), Role::Plain)
        .button(t("Nicht sichern"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .run_modal();
    if response == NSAlertFirstButtonReturn {
        save_for(&view, again)
    } else if response == NSAlertSecondButtonReturn {
        give_up(&view);
        Answer::Yes
    } else {
        Answer::No
    }
}

/// Saves `view` for a close, which `again` takes up once the file is written and nothing is left unsaved.
pub(super) fn save_for(view: &EditorTextView, again: &Again) -> Answer {
    let again = Rc::clone(again);
    saving::save_then(
        view,
        false,
        Box::new(move |view, written| again(written && !view.document().is_dirty())),
    );
    Answer::Later
}

/// The user let the text of `view` go: a close that asks again once another editor saved lets it close at once.
pub(super) fn give_up(view: &EditorTextView) {
    recovery::discard(view);
    view.ivars().file.given_up.set(true);
}

/// Whether the app may quit: the user saved or gave up the text of each editor whose file is gone, one question each,
/// then saved or dropped the other unsaved changes; `Later` while saves run, which `again` hears of. When the user
/// cancels, every editor keeps its copy.
pub(crate) fn confirm_quit(mtm: MainThreadMarker, again: &Again) -> Answer {
    if term_base::unattended::is() {
        open_editors().iter().for_each(|view| give_up(view));
        return Answer::Yes;
    }
    let mut answer = Answer::Yes;
    for view in open_editors().iter().filter(|view| conflict::is_gone(view)) {
        if view.ivars().file.given_up.get() {
            continue;
        }
        answer = conflict::may_close(view, again);
        if answer != Answer::Yes {
            break;
        }
    }
    if answer == Answer::Yes {
        answer = unsaved_may_go(mtm, again);
    }
    if answer == Answer::No {
        keep_copies();
    }
    answer
}

/// Every editor keeps its copy and is asked again next time, as after a quit the user cancelled.
pub(crate) fn keep_copies() {
    for view in open_editors() {
        view.ivars().file.given_up.set(false);
        recovery::store(&view);
    }
}

fn unsaved_may_go(mtm: MainThreadMarker, again: &Again) -> Answer {
    let count = dirty_count();
    if count == 0 {
        return Answer::Yes;
    }
    let response = Alert::new(
        mtm,
        &tf("{} Dateien haben ungesicherte Änderungen.", &[&count.to_string()]),
    )
    .informative(t("Ohne Sichern gehen deine Änderungen verloren."))
    .button(t("Alle sichern"), Role::Plain)
    .button(t("Nicht sichern"), Role::Plain)
    .button(t("Abbrechen"), Role::Plain)
    .run_modal();
    if response == NSAlertFirstButtonReturn {
        save_each(unsaved(), Rc::clone(again))
    } else if response == NSAlertSecondButtonReturn {
        for view in open_editors() {
            recovery::discard(&view);
        }
        Answer::Yes
    } else {
        Answer::No
    }
}

/// The names of the documents in `controller`'s window whose text closing would lose: unsaved changes, and files gone
/// from disk (E43).
pub(crate) fn unsaved_in(controller: &Workspace) -> Vec<String> {
    editors_in(controller)
        .into_iter()
        .filter(|view| view.document().is_dirty() || conflict::is_gone(view))
        .map(|view| {
            view.document()
                .path()
                .map_or_else(|| t("Ohne Titel").to_string(), display_name)
        })
        .collect()
}

/// Gives up the changes in the editors of `controller`'s window and their crash copies, as "Nicht sichern" does.
pub(crate) fn discard_changes(controller: &Workspace) {
    for view in editors_in(controller) {
        recovery::discard(&view);
    }
}

/// How many open editors have unsaved changes, those of gone files aside, which get a question of their own.
pub(crate) fn dirty_count() -> usize {
    unsaved().len()
}

/// The files below `root` whose open editors have unsaved changes, those of gone files aside.
pub(crate) fn unsaved_below(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = unsaved()
        .iter()
        .filter_map(|view| {
            view.document()
                .path()
                .filter(|p| p.starts_with(root))
                .map(Path::to_path_buf)
        })
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Saves the editors of files below `root` with unsaved changes and calls `then` once the writes are over, with whether
/// none is left unsaved.
pub(crate) fn save_below(root: &Path, then: Box<dyn FnOnce(bool)>) {
    let below = |view: &Retained<EditorTextView>| view.document().path().is_some_and(|p| p.starts_with(root));
    let views: Vec<_> = unsaved().into_iter().filter(below).collect();
    let then = Cell::new(Some(then));
    let root = root.to_path_buf();
    let done: Again = Rc::new(move |_| {
        let left = unsaved()
            .iter()
            .any(|view| view.document().path().is_some_and(|p| p.starts_with(&root)));
        if let Some(then) = then.take() {
            then(!left);
        }
    });
    if save_each(views, Rc::clone(&done)) == Answer::Yes {
        done(true);
    }
}

/// Saves every editor with unsaved changes, those of gone files aside, and calls `then` once the writes are over, with
/// whether none is left unsaved.
pub(crate) fn save_every(then: Box<dyn FnOnce(bool)>) {
    let then = Cell::new(Some(then));
    let done: Again = Rc::new(move |_| {
        let left = !unsaved().is_empty();
        if let Some(then) = then.take() {
            then(!left);
        }
    });
    if save_each(unsaved(), Rc::clone(&done)) == Answer::Yes {
        done(true);
    }
}

/// Saves each of `views`; `Yes` when there is none, else `Later`, and `then` hears once every write is over whether
/// all of them wrote their files and left nothing unsaved.
pub(crate) fn save_each(views: Vec<Retained<EditorTextView>>, then: Again) -> Answer {
    if views.is_empty() {
        return Answer::Yes;
    }
    let left = Rc::new(Cell::new(views.len()));
    let all = Rc::new(Cell::new(true));
    for view in &views {
        let (left, all, then) = (Rc::clone(&left), Rc::clone(&all), Rc::clone(&then));
        saving::save_then(
            view,
            false,
            Box::new(move |view, written| {
                all.set(all.get() && written && !view.document().is_dirty());
                left.set(left.get() - 1);
                if left.get() == 0 {
                    then(all.get());
                }
            }),
        );
    }
    Answer::Later
}

pub(super) fn unsaved() -> Vec<Retained<EditorTextView>> {
    open_editors()
        .into_iter()
        .filter(|view| view.document().is_dirty() && !conflict::is_gone(view))
        .collect()
}
