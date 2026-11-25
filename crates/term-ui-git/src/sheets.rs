//! The sheets of the Git panel and the diff tab: confirmations, git's output and the name of a new branch.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSFont, NSScrollView, NSTextField, NSTextView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_git::PullMode;
use term_ui::kit::alert::{Alert, Role, first};

use term_ui::kit::ns;

/// Asks before something that cannot be undone; `then` runs when the user agrees.
pub(crate) fn confirm(window: &NSWindow, message: &str, info: &str, action: &str, then: impl FnOnce() + 'static) {
    Alert::new(window.mtm(), message)
        .warning()
        .informative(info)
        .button(action, Role::Destructive)
        .button(t("Abbrechen"), Role::Plain)
        .sheet(window, agreed(then));
}

/// Asks before something is deleted for good; the destructive button is not the default, Return cancels.
pub fn confirm_delete(window: &NSWindow, message: &str, info: &str, action: &str, then: impl FnOnce() + 'static) {
    Alert::new(window.mtm(), message)
        .warning()
        .informative(info)
        .button(action, Role::Destructive)
        .button(t("Abbrechen"), Role::CancelOnReturn)
        .sheet(window, agreed(then));
}

/// Offers to save the open documents `names` before a stash action, which sees only the files on disk; `then` runs
/// when the user chooses to save.
pub(crate) fn save_first(window: &NSWindow, names: &[String], then: impl FnOnce() + 'static) {
    let message = match names {
        [one] => tf("„{}“ hat ungesicherte Änderungen.", &[one]),
        many => tf("{} Dateien haben ungesicherte Änderungen.", &[&many.len().to_string()]),
    };
    Alert::new(window.mtm(), &message)
        .informative(t(
            "Der Stash sieht nur, was auf der Platte steht. Sichere zuerst, damit deine Änderungen dabei sind.",
        ))
        .button(t("Sichern"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .sheet(window, agreed(then));
}

/// Asks for the message of a new stash entry and whether untracked files go along; `then` gets both.
pub(crate) fn stash_options(window: &NSWindow, then: impl FnOnce(String, bool) + 'static) {
    let mtm = window.mtm();
    let field = NSTextField::textFieldWithString(&ns(""), mtm);
    field.setPlaceholderString(Some(&ns(t("Nachricht (freiwillig)"))));
    term_ui::kit::set_label(&field, Some(t("Nachricht (freiwillig)")));
    // SAFETY: Without a target and an action the checkbox sends nothing; its state is read when the sheet ends.
    let untracked = unsafe {
        objc2_app_kit::NSButton::checkboxWithTitle_target_action(
            &ns(t("Mit nicht versionierten Dateien")),
            None,
            None,
            mtm,
        )
    };
    let stack = objc2_app_kit::NSStackView::new(mtm);
    stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    stack.setSpacing(term_ui::kit::style::space::M);
    stack.addArrangedSubview(&field);
    stack.addArrangedSubview(&untracked);
    field.widthAnchor().constraintEqualToConstant(260.0).setActive(true);
    stack.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 54.0)));
    let alert = Alert::new(mtm, t("Änderungen stashen"))
        .informative(t(
            "Die Änderungen verlassen die Arbeitskopie und lassen sich später wieder anwenden.",
        ))
        .button(t("Stashen"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&stack)
        .focus(&field);
    let typed = field.clone();
    alert.sheet(window, move |response| {
        if first(response) {
            let with = untracked.state() == objc2_app_kit::NSControlStateValueOn;
            then(typed.stringValue().to_string().trim().to_string(), with);
        }
    });
}

/// Shows what git or a hook printed.
pub(crate) fn output(window: &NSWindow, message: &str, text: &str) {
    Alert::new(window.mtm(), message)
        .button(t("OK"), Role::Plain)
        .accessory(&text_box(window, text))
        .sheet(window, |_| {});
}

/// Asks how to join a branch that diverged from its upstream; `then` gets the choice, `None` when the user cancels.
pub(crate) fn pull_mode(window: &NSWindow, then: impl FnOnce(Option<PullMode>) + 'static) {
    Alert::new(
        window.mtm(),
        t("Lokaler und entfernter Branch sind auseinandergelaufen"),
    )
    .informative(t(
        "Zusammenführen legt einen Merge-Commit an; Rebase setzt deine Commits auf die neuen des Remotes. Beides verwirft nichts.",
    ))
    .button(t("Zusammenführen"), Role::Plain)
    .button(t("Rebase"), Role::Plain)
    .button(t("Abbrechen"), Role::Plain)
    .sheet(window, move |response| {
        then(match response {
            r if r == NSAlertFirstButtonReturn => Some(PullMode::Merge),
            r if r == NSAlertSecondButtonReturn => Some(PullMode::Rebase),
            _ => None,
        });
    });
}

/// Asks, under `title`, for the name of a new branch that starts where `info` says; `then` gets it unless the user
/// cancels or leaves it empty.
pub(crate) fn branch_name(window: &NSWindow, title: &str, info: &str, then: impl FnOnce(String) + 'static) {
    let mtm = window.mtm();
    let field = NSTextField::textFieldWithString(&ns(""), mtm);
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)));
    field.setPlaceholderString(Some(&ns(t("feature/name"))));
    let alert = Alert::new(mtm, title)
        .informative(info)
        .button(t("Anlegen"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&field)
        .focus(&field);
    let typed = field.clone();
    alert.sheet(window, move |response| {
        let name = typed.stringValue().to_string().trim().to_string();
        if first(response) && !name.is_empty() {
            then(name);
        }
    });
}

/// The completion of a sheet that runs `then` when its first button ended it.
fn agreed(then: impl FnOnce() + 'static) -> impl FnOnce(objc2_app_kit::NSModalResponse) + 'static {
    move |response| {
        if first(response) {
            then();
        }
    }
}

/// A scrolling box of monospaced text that can be selected and copied.
fn text_box(window: &NSWindow, text: &str) -> Retained<NSScrollView> {
    let mtm = window.mtm();
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 200.0));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(true);
    scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);
    let view = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    view.setEditable(false);
    view.setSelectable(true);
    view.setRichText(false);
    view.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        term_ui::kit::style::SMALL,
        term_render::constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    view.setString(&ns(text));
    view.setHorizontallyResizable(true);
    view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
    view.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
    scroll.setDocumentView(Some(&view));
    scroll
}
