//! Alerts, built in one place: the message, its explanation, the buttons in their order with what each one does, and
//! whether the alert runs on its own or as a sheet on a window.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSButton, NSModalResponse, NSView, NSWindow};

use super::{guarded, ns};

/// What a button of an alert does besides answering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Answers with its place; the first button takes Return, as AppKit gives it.
    Plain,
    /// Loses work or data, which AppKit shows on the button.
    Destructive,
    /// Cancels and takes Return from the first button, for a question whose safe answer is not to go on.
    CancelOnReturn,
    /// Cancels and answers Escape.
    CancelOnEscape,
}

/// An alert being built; `run_modal` or `sheet` shows it.
pub struct Alert {
    alert: Retained<NSAlert>,
    first: Option<Retained<NSButton>>,
}

impl Alert {
    /// An alert that says `message`.
    pub fn new(mtm: MainThreadMarker, message: &str) -> Alert {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&ns(message));
        Alert { alert, first: None }
    }

    /// The alert of a warning.
    pub fn warning(self) -> Alert {
        self.alert.setAlertStyle(NSAlertStyle::Warning);
        self
    }

    /// The alert of something that can do harm, such as a secret about to leave the computer.
    pub fn critical(self) -> Alert {
        self.alert.setAlertStyle(NSAlertStyle::Critical);
        self
    }

    /// `text` below the message, which says more about it.
    pub fn informative(self, text: &str) -> Alert {
        self.alert.setInformativeText(&ns(text));
        self
    }

    /// `view` below the texts, such as a field to type into.
    pub fn accessory(self, view: &NSView) -> Alert {
        self.alert.setAccessoryView(Some(view));
        self
    }

    /// Gives `view`, part of the accessory, the keyboard focus when the alert shows.
    pub fn focus(self, view: &NSView) -> Alert {
        self.alert.window().setInitialFirstResponder(Some(view));
        self
    }

    /// A button titled `title` after those added before, which does what `role` says.
    pub fn button(mut self, title: &str, role: Role) -> Alert {
        let button = self.alert.addButtonWithTitle(&ns(title));
        match role {
            Role::Plain => {}
            Role::Destructive => button.setHasDestructiveAction(true),
            Role::CancelOnReturn => {
                button.setKeyEquivalent(&ns("\r"));
                if let Some(first) = &self.first {
                    first.setKeyEquivalent(&ns(""));
                }
            }
            Role::CancelOnEscape => button.setKeyEquivalent(&ns("\u{1b}")),
        }
        if self.first.is_none() {
            self.first = Some(button);
        }
        self
    }

    /// The alert, for what the builder does not cover, such as laying it out before it shows.
    pub fn ns(&self) -> &NSAlert {
        &self.alert
    }

    /// Shows the alert in a window of its own and waits for the answer: `NSAlertFirstButtonReturn` and on for the
    /// buttons in their order.
    pub fn run_modal(self) -> NSModalResponse {
        self.alert.runModal()
    }

    /// Shows the alert as a sheet on `window`; `then` gets the answer, as `run_modal` gives it.
    pub fn sheet(self, window: &NSWindow, then: impl FnOnce(NSModalResponse) + 'static) {
        let then = std::cell::Cell::new(Some(then));
        let handler = block2::RcBlock::new(move |response: NSModalResponse| {
            guarded((), || {
                if let Some(then) = then.take() {
                    then(response);
                }
            });
        });
        self.alert
            .beginSheetModalForWindow_completionHandler(window, Some(&handler));
    }
}

/// Whether `response`, the answer to an alert, is its first button.
pub fn first(response: NSModalResponse) -> bool {
    response == NSAlertFirstButtonReturn
}

/// Shows a warning that says `message` and `detail` and waits for OK.
pub fn warn(mtm: MainThreadMarker, message: &str, detail: &str) {
    Alert::new(mtm, message).warning().informative(detail).run_modal();
}

/// Asks with a warning whether to do `action`, which `message` and `detail` describe, or cancel; whether the user
/// chose `action`.
pub fn confirm(mtm: MainThreadMarker, message: &str, detail: &str, action: &str) -> bool {
    first(
        Alert::new(mtm, message)
            .warning()
            .informative(detail)
            .button(action, Role::Plain)
            .button(splat_i18n::t("Abbrechen"), Role::Plain)
            .run_modal(),
    )
}
