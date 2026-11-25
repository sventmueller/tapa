//! Asking before ending programs (F213), and quitting: every shell gets its hang-up and the time to end.

use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle};
use splat_i18n::{t, tf};
use term_ui::kit::ns;

use crate::state::try_app;

/// How long each shell gets at each step of ending: hang-up, SIGTERM, SIGKILL.
const GRACE: Duration = Duration::from_millis(500);

/// An alert as macOS writes one: the app icon, `message` in bold, `detail` below, `confirm` as the default button,
/// marked destructive, and Cancel beside it, which Escape chooses.
fn alert(mtm: MainThreadMarker, message: &str, detail: &str, confirm: &str) -> Retained<NSAlert> {
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&ns(message));
    alert.setInformativeText(&ns(detail));
    alert.addButtonWithTitle(&ns(confirm)).setHasDestructiveAction(true);
    alert
        .addButtonWithTitle(&ns(t("Abbrechen")))
        .setKeyEquivalent(&ns("\u{1b}"));
    alert
}

/// Whether the user agrees to `confirm`, a destructive step, after reading `message` and `detail`.
pub(crate) fn confirm(mtm: MainThreadMarker, message: &str, detail: &str, confirm: &str) -> bool {
    alert(mtm, message, detail, confirm).runModal() == NSAlertFirstButtonReturn
}

/// Whether the user agrees to end running programs with `confirm`; the alert offers not to ask again, which turns
/// off the setting that asks (F213).
pub(crate) fn ask(mtm: MainThreadMarker, message: &str, detail: &str, confirm: &str) -> bool {
    let alert = alert(mtm, message, detail, confirm);
    alert.setShowsSuppressionButton(true);
    if let Some(suppression) = alert.suppressionButton() {
        suppression.setTitle(&ns(t("Nicht mehr fragen")));
    }
    let agreed = alert.runModal() == NSAlertFirstButtonReturn;
    let suppressed = alert
        .suppressionButton()
        .is_some_and(|button| button.state() == objc2_app_kit::NSControlStateValueOn);
    if agreed && suppressed {
        let written = crate::settings::apply(|doc| {
            doc.set(&["window", "confirm_close"], false);
            Ok(true)
        });
        if let Err(error) = written {
            term_base::diag::report("confirm_close", error);
        }
    }
    agreed
}

/// Whether the app may quit: no program runs besides the shells, or the user agreed.
pub(crate) fn may_quit(mtm: MainThreadMarker) -> bool {
    let Some(app) = try_app() else { return true };
    let busy: Vec<String> = app.windows.borrow().iter().flat_map(|window| window.busy()).collect();
    if busy.is_empty() || !app.config().window.confirm_close {
        return true;
    }
    if term_base::unattended::is() {
        #[cfg(feature = "selftest")]
        crate::selftest::menus::asked(&busy);
        return true;
    }
    ask(
        mtm,
        &tf("{} beenden?", &[term_product::NAME]),
        &tf(
            "Diese Programme laufen noch und werden beendet: {}",
            &[&busy.join(", ")],
        ),
        t("Beenden"),
    )
}

/// Whether the app quits now: no program runs or the user agreed, and the editors' unsaved text is saved or given
/// up; while saves the user asked for run, the answer comes once they are over.
pub(crate) fn should_terminate(mtm: MainThreadMarker) -> objc2_app_kit::NSApplicationTerminateReply {
    use objc2_app_kit::NSApplicationTerminateReply as Reply;
    if !may_quit(mtm) {
        return Reply::TerminateCancel;
    }
    let again: term_ui_editor::Again = std::rc::Rc::new(move |saved| {
        term_base::main_soon(move || {
            let Some(mtm) = MainThreadMarker::new() else { return };
            if saved {
                leave();
            }
            objc2_app_kit::NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(saved);
        });
    });
    match term_ui_editor::confirm_quit(mtm, &again) {
        term_ui_editor::Answer::Yes => {
            leave();
            Reply::TerminateNow
        }
        term_ui_editor::Answer::No => Reply::TerminateCancel,
        term_ui_editor::Answer::Later => Reply::TerminateLater,
    }
}

/// What the app does as it goes: stores its windows, drops the editors' copies of text now saved or given up, and
/// ends every shell and agent.
fn leave() {
    crate::restore::save_at_quit();
    LEFT.store(true, std::sync::atomic::Ordering::Relaxed);
    term_ui_editor::quit().finish(COPIES_LIMIT);
    end_all();
}

/// Set once the app stored its windows to quit, so that tabs closing as their shells end store nothing over them.
static LEFT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the app stored its windows to quit.
pub(crate) fn left() -> bool {
    LEFT.load(std::sync::atomic::Ordering::Relaxed)
}

/// How long quitting waits for the editors' copies to be removed.
const COPIES_LIMIT: Duration = Duration::from_secs(2);

/// Ends every shell and agent before the app goes, so that no process the app started outlives it.
pub(crate) fn end_all() {
    crate::agent::quit();
    term_process::Supervisor::global().shutdown(GRACE);
}
