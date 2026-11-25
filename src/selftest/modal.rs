//! The self-test's steps for what asks in a modal alert or save panel, which no other step can answer:
//! `modal:<answer>=<step>` runs `step` from the run loop, so that the main queue keeps running while its question
//! shows, then prints the question, takes the picture `modal` of an alert and answers it with the button titled
//! `answer`; a save panel is answered `ok` or `cancel` (`saveasname:<path>` first has it offer that file). `attended:on` has the app ask the
//! questions the self-test otherwise answers itself, such as whether to save before closing, `attended:off` stops
//! that again before the end, whose quit would ask too. `activate` brings the app to the front, which actions sent
//! along the responder chain, such as `save_as`, need; `send:<selector>` sends such an action, `termSaveAs:` for
//! one, to the front window's first responder without it.

use std::time::Duration;

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSButton, NSModalResponseCancel, NSModalResponseOK, NSSavePanel, NSTextField, NSView, NSWindow,
};

/// How long the step waits for the question, its answer and what follows.
pub(super) const TIME: Duration = Duration::from_millis(3500);

/// How long the question shows before it is answered.
const SHOWN: Duration = Duration::from_millis(1500);

/// Runs `step` if it is `attended:`, `activate`, `send:` or `saveasname:`; whether it was.
pub(super) fn other(step: &str) -> bool {
    if let Some(on) = step.strip_prefix("attended:") {
        if on == "on" {
            term_base::unattended::unset();
        } else {
            term_base::unattended::set();
        }
        println!("attended {on}");
    } else if step == "activate" {
        activate();
    } else if let Some(selector) = step.strip_prefix("send:") {
        send(selector);
    } else if let Some(path) = step.strip_prefix("saveasname:") {
        term_ui_editor::selftest::offer_in_save_panel(std::path::Path::new(path));
    } else {
        return false;
    }
    true
}

/// Runs `step` if it is `modal:`; whether it was.
pub(super) fn run(step: &str) -> bool {
    let Some(rest) = step.strip_prefix("modal:") else {
        return false;
    };
    let Some((answer, inner)) = rest.split_once('=') else {
        eprintln!("selftest: modal needs <answer>=<step>, not {rest}");
        return true;
    };
    let (answer, inner) = (answer.to_string(), inner.to_string());
    let shown = inner.clone();
    term_base::main_after(SHOWN, move || respond(&shown, &answer));
    term_base::main_thread::main_on_run_loop(move || {
        if !super::perform(&inner) {
            eprintln!("selftest: unknown step {inner}");
        }
        println!("modal {inner}: step over");
    });
    true
}

/// Brings the app and its front window to the front, as actions sent along the responder chain need a key window.
fn activate() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    #[expect(
        deprecated,
        reason = "the self-test must take the keyboard even while another app is in front"
    )]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    let window = crate::state::app().front().and_then(|window| window.front_ns_window());
    if let Some(window) = &window {
        window.makeKeyAndOrderFront(None);
    }
    println!("activate: key {}", window.is_some_and(|window| window.isKeyWindow()));
}

/// Sends the action `selector` to the first responder of the front window, as its menu item does while the window is
/// key, which it is not while another app is in front.
fn send(selector: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let window = crate::state::app().front().and_then(|window| window.front_ns_window());
    let Some(responder) = window.and_then(|window| window.firstResponder()) else {
        return println!("send {selector}: no first responder");
    };
    let Ok(name) = std::ffi::CString::new(selector) else {
        return;
    };
    let target: &objc2::runtime::AnyObject = &responder;
    // SAFETY: The self-test sends only actions, which take the sender, here none.
    let sent = unsafe {
        NSApplication::sharedApplication(mtm).sendAction_to_from(
            objc2::runtime::Sel::register(&name),
            Some(target),
            None,
        )
    };
    println!("send {selector}: {sent}");
}

fn respond(step: &str, answer: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.modalWindow() else {
        return println!("modal {step}: nothing asks");
    };
    if let Some(panel) = window.downcast_ref::<NSSavePanel>() {
        return save_panel(step, panel, answer);
    }
    let mut texts = Vec::new();
    if let Some(view) = window.contentView() {
        collect(&view, &mut texts);
    }
    println!("modal {step}: asks {texts:?}");
    let dir = std::env::var_os("TAPA_SELFTEST_DIR").map_or_else(|| "target/selftest".into(), std::path::PathBuf::from);
    super::menus::picture(&window, &dir.join("modal.png"));
    match button(&window, answer) {
        Some(button) => {
            println!("modal {step}: answers {answer}");
            // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
            unsafe { button.performClick(None) };
        }
        None => {
            println!("modal {step}: no button {answer}, cancelled");
            app.abortModal();
        }
    }
}

fn save_panel(step: &str, panel: &NSSavePanel, answer: &str) {
    let folder = panel
        .directoryURL()
        .and_then(|url| url.path())
        .map(|path| path.to_string());
    println!(
        "modal {step}: save panel offers {:?} in {folder:?}, answers {answer}",
        panel.nameFieldStringValue().to_string()
    );
    // The panel's window is another process's, whose buttons `ok:` and `cancel:` do not reach.
    let code = if answer == "ok" {
        NSModalResponseOK
    } else {
        NSModalResponseCancel
    };
    NSApplication::sharedApplication(objc2::MainThreadOnly::mtm(panel)).stopModalWithCode(code);
}

fn button(window: &NSWindow, title: &str) -> Option<objc2::rc::Retained<NSButton>> {
    fn find(view: &NSView, title: &str) -> Option<objc2::rc::Retained<NSButton>> {
        if let Some(button) = view.downcast_ref::<NSButton>()
            && button.title().to_string() == title
        {
            return Some(objc2::Message::retain(button));
        }
        view.subviews().iter().find_map(|child| find(&child, title))
    }
    find(&*window.contentView()?, title)
}

fn collect(view: &NSView, out: &mut Vec<String>) {
    if let Some(button) = view.downcast_ref::<NSButton>() {
        out.push(button.title().to_string());
    } else if let Some(field) = view.downcast_ref::<NSTextField>() {
        out.push(field.stringValue().to_string());
    }
    for child in view.subviews().iter() {
        collect(&child, out);
    }
}
