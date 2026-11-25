//! ssh's questions while a remote workspace connects (E25): a password from the keychain the first time, else a sheet
//! at the workspace's window; an unknown host key is shown and only taken on the user's word, never silently.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSSecureTextField, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_remote::askpass::{Kind, Question};
use term_ui::kit::alert::{Alert, Role};
use term_ui::kit::ns;

use super::{Remote, account, by_token, keep_password, ns_window_of, secrets};

const FIELD_WIDTH: f64 = 300.0;
const FIELD_HEIGHT: f64 = 24.0;

/// A sheet that asks and its field, if it has one.
type Asking = (Retained<objc2_app_kit::NSAlert>, Option<Retained<NSTextField>>);

thread_local! {
    /// The sheet that asks now and its field, for the self-test.
    static ASKING: RefCell<Option<Asking>> = const { RefCell::new(None) };
}

/// Answers `question`: the kept password once per opening, else whatever the user says.
pub(super) fn answer(question: Question) {
    let Some(remote) = by_token(&question.token) else {
        question.answer(None);
        return;
    };
    let kind = term_remote::askpass::kind(&question.prompt);
    if kind == Kind::Secret && !remote.tried_password.replace(true) {
        let kept = secrets().get(&account(&remote.target())).ok().flatten();
        if let Some(password) = kept {
            question.answer(Some(password));
            return;
        }
    }
    ask_user(&remote, kind, question);
}

fn ask_user(remote: &Rc<Remote>, kind: Kind, question: Question) {
    let Some(window) = ns_window_of(remote) else {
        question.answer(None);
        return;
    };
    let mtm = window.mtm();
    let target = remote.target();
    let place = target.account();
    let (message, field, keep) = match kind {
        Kind::Secret => {
            let field: Retained<NSTextField> =
                NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), frame()).into_super();
            // SAFETY: Without a target and an action the checkbox only keeps its state, which the answer reads.
            let keep = unsafe {
                NSButton::checkboxWithTitle_target_action(&ns(t("Im Schlüsselbund sichern")), None, None, mtm)
            };
            (tf("Passwort für {}", &[&place]), Some(field), Some(keep))
        }
        Kind::HostKey => (tf("Unbekannter Host-Schlüssel bei {}", &[&place]), None, None),
        Kind::Text => (
            tf("Frage von {}", &[&place]),
            Some(NSTextField::initWithFrame(NSTextField::alloc(mtm), frame())),
            None,
        ),
    };
    let mut alert = Alert::new(mtm, &message)
        .informative(question.prompt.trim())
        .button(t("Verbinden"), Role::Plain)
        .button(t("Abbrechen"), Role::CancelOnEscape);
    if kind == Kind::HostKey {
        alert = alert.warning();
    }
    if let Some(field) = &field {
        let height = FIELD_HEIGHT + if keep.is_some() { FIELD_HEIGHT + 6.0 } else { 0.0 };
        let stack = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(FIELD_WIDTH, height)),
        );
        field.setFrameOrigin(NSPoint::new(0.0, height - FIELD_HEIGHT));
        stack.addSubview(field);
        if let Some(keep) = &keep {
            keep.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(FIELD_WIDTH, FIELD_HEIGHT),
            ));
            stack.addSubview(keep);
        }
        alert = alert.accessory(&stack);
        alert.ns().layout();
        alert = alert.focus(field);
    }
    ASKING.with(|asking| *asking.borrow_mut() = Some((objc2::Message::retain(alert.ns()), field.clone())));
    alert.sheet(&window, move |response| {
        ASKING.with(|asking| asking.borrow_mut().take());
        if !term_ui::kit::alert::first(response) {
            question.answer((kind == Kind::HostKey).then(|| "no".to_string()));
            return;
        }
        let answer = match &field {
            Some(field) => field.stringValue().to_string(),
            None => "yes".to_string(),
        };
        if keep.is_some_and(|keep| keep.state() == objc2_app_kit::NSControlStateValueOn) {
            keep_password(&target, &answer);
        }
        question.answer(Some(answer));
    });
}

fn frame() -> NSRect {
    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(FIELD_WIDTH, FIELD_HEIGHT))
}

/// Answers the question a sheet asks now with `text`, as typing it and the first button would, or cancels with
/// `None`; its heading, if one asked.
#[cfg(feature = "selftest")]
pub(crate) fn answer_shown(text: Option<&str>) -> Option<String> {
    let (alert, field) = ASKING.with(|asking| asking.borrow().clone())?;
    let heading = alert.messageText().to_string();
    if let (Some(field), Some(text)) = (&field, text) {
        field.setStringValue(&ns(text));
    }
    let buttons = alert.buttons();
    let index = if text.is_some() { 0 } else { 1 };
    if index < buttons.count() {
        // SAFETY: A click without a sender, which ends the sheet as the mouse would.
        unsafe { buttons.objectAtIndex(index).performClick(None) };
    }
    Some(heading)
}
