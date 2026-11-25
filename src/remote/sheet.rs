//! The sheet that asks where a remote workspace works (E25): the host, from `~/.ssh/config` or typed, the port and
//! user where they differ from ssh's own, an optional password for the keychain, and the folder there.

use std::cell::RefCell;

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSComboBox, NSSecureTextField, NSTextField, NSView, NSWindow};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use splat_i18n::t;
use term_remote::Target;
use term_ui::kit::alert::{Alert, Role, first};
use term_ui::kit::ns;

const FIELD_WIDTH: f64 = 320.0;
const FIELD_HEIGHT: f64 = 24.0;
const GAP: f64 = 8.0;

/// A sheet that asks and its fields.
type Asking = (Retained<objc2_app_kit::NSAlert>, Vec<Retained<NSTextField>>);

thread_local! {
    /// The sheet that asks now and its fields, for the self-test.
    static ASKING: RefCell<Option<Asking>> = const { RefCell::new(None) };
}

/// Asks for a remote workspace's machine in a sheet of `window`, filled with `current`; `done` gets the target and the
/// password typed, which may be empty. A host or user ssh would misread asks again.
pub(crate) fn ask(window: &NSWindow, current: Option<&Target>, done: impl FnOnce(Target, String) + 'static) {
    let mtm = window.mtm();
    let fields = fields(mtm, current);
    let stack = stack(mtm, &fields);
    let (heading, action) = if current.is_some() {
        (t("Verbindung bearbeiten"), t("Sichern"))
    } else {
        (t("Entfernter Arbeitsbereich"), t("Verbinden"))
    };
    let alert = Alert::new(mtm, heading)
        .informative(t(
            "Terminals und Agenten laufen auf dem Rechner, den ssh so erreicht; Dateiliste und Git gibt es dort nicht.",
        ))
        .accessory(&stack)
        .button(action, Role::Plain)
        .button(t("Abbrechen"), Role::CancelOnEscape);
    alert.ns().layout();
    let alert = alert.focus(&fields[0]);
    ASKING.with(|asking| *asking.borrow_mut() = Some((objc2::Message::retain(alert.ns()), fields.clone())));
    let window = objc2::Message::retain(window);
    let edited = current.is_some();
    alert.sheet(&window.clone(), move |response| {
        ASKING.with(|asking| asking.borrow_mut().take());
        if !first(response) {
            return;
        }
        let text = |index: usize| fields[index].stringValue().to_string().trim().to_string();
        let target = Target {
            host: text(0),
            port: text(1).parse().ok(),
            user: Some(text(2)).filter(|user| !user.is_empty()),
            folder: text(4),
        };
        let password = fields[3].stringValue().to_string();
        if target.is_valid() {
            done(target, password);
            return;
        }
        refuse(&window, edited.then_some(target), done);
    });
}

/// Says why the host or user cannot be taken, then asks again, filled with `again` where an edit is refused.
fn refuse(window: &NSWindow, again: Option<Target>, done: impl FnOnce(Target, String) + 'static) {
    Alert::new(window.mtm(), t("Host oder Benutzer ungültig"))
        .warning()
        .informative(t(
            "Beide dürfen nicht leer sein, nicht mit einem Bindestrich beginnen und keine Leerzeichen oder @ enthalten.",
        ))
        .button(t("OK"), Role::Plain)
        .sheet(window, move |_| {
            super::later(move || {
                if let Some(window) = crate::state::try_app().and_then(|app| app.front()?.front_ns_window()) {
                    ask(&window, again.as_ref(), done);
                }
            });
        });
}

/// The fields for host, port, user, password and folder, filled with `current`.
fn fields(mtm: objc2::MainThreadMarker, current: Option<&Target>) -> Vec<Retained<NSTextField>> {
    let frame = || NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(FIELD_WIDTH, FIELD_HEIGHT));
    let host = NSComboBox::initWithFrame(NSComboBox::alloc(mtm), frame());
    let hosts: Vec<Retained<objc2::runtime::AnyObject>> = term_suggest::ssh_hosts(&term_base::home())
        .iter()
        .map(|host| ns(host).into())
        .collect();
    // SAFETY: The items are strings, which the combo box shows and completes from.
    unsafe { host.addItemsWithObjectValues(&NSArray::from_retained_slice(&hosts)) };
    host.setCompletes(true);
    let host: Retained<NSTextField> = host.into_super();
    let port = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame());
    let user = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame());
    let password: Retained<NSTextField> =
        NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), frame()).into_super();
    let folder = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame());
    let fields = vec![host, port, user, password, folder];
    let placeholders = [
        t("Host, aus ~/.ssh/config oder eine Adresse"),
        t("Port (leer: wie ssh)"),
        t("Benutzer (leer: wie ssh)"),
        if current.is_some() {
            t("Passwort (leer: unverändert)")
        } else {
            t("Passwort (optional, im Schlüsselbund)")
        },
        t("Ordner dort (leer: Home-Ordner)"),
    ];
    for (field, placeholder) in fields.iter().zip(placeholders) {
        field.setPlaceholderString(Some(&ns(placeholder)));
        term_ui::kit::set_label(field, Some(placeholder));
    }
    if let Some(current) = current {
        fields[0].setStringValue(&ns(&current.host));
        fields[1].setStringValue(&ns(&current.port.map(|port| port.to_string()).unwrap_or_default()));
        fields[2].setStringValue(&ns(current.user.as_deref().unwrap_or_default()));
        fields[4].setStringValue(&ns(&current.folder));
    }
    fields
}

/// `fields` below each other in one view, Tab stepping from one to the next.
fn stack(mtm: objc2::MainThreadMarker, fields: &[Retained<NSTextField>]) -> Retained<NSView> {
    let count = fields.len() as f64;
    let height = count * FIELD_HEIGHT + (count - 1.0) * GAP;
    let stack = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(FIELD_WIDTH, height)),
    );
    for (index, field) in fields.iter().enumerate() {
        field.setFrameOrigin(NSPoint::new(
            0.0,
            height - (index as f64 + 1.0) * FIELD_HEIGHT - index as f64 * GAP,
        ));
        stack.addSubview(field);
    }
    for pair in fields.windows(2) {
        // SAFETY: Both fields live in the same accessory view, which keeps them for as long as the sheet shows.
        unsafe { pair[0].setNextKeyView(Some(&pair[1])) };
    }
    stack
}

/// Fills the sheet that asks now with `values` for host, port, user, password and folder and presses its first
/// button, or with `None` its second; whether one asked.
#[cfg(feature = "selftest")]
pub(crate) fn fill_shown(values: Option<&[&str]>) -> bool {
    let Some((alert, fields)) = ASKING.with(|asking| asking.borrow().clone()) else {
        return false;
    };
    if let Some(values) = values {
        for (field, value) in fields.iter().zip(values) {
            field.setStringValue(&ns(value));
        }
    }
    let buttons = alert.buttons();
    let index = if values.is_some() { 0 } else { 1 };
    if index < buttons.count() {
        // SAFETY: A click without a sender, which ends the sheet as the mouse would.
        unsafe { buttons.objectAtIndex(index).performClick(None) };
    }
    true
}
