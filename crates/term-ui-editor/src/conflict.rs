//! The bar above an editor whose file changed on disk while it held unsaved text, or whose file is gone (E24). It
//! offers the differences, a three-way merge against the version the text started from, the file's version and the
//! editor's own, and stays until one is chosen; typing goes on meanwhile, and a further change of the file updates it.

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAlertFirstButtonReturn, NSAlertSecondButtonReturn, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor,
    NSControlSize, NSImageView, NSLayoutConstraint, NSLayoutConstraintOrientation, NSLineBreakMode, NSStackView,
    NSStackViewGravity, NSTextField, NSTitlePosition, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSRange, NSSize};
use splat_i18n::{t, tf};
use term_document::merge::{self, Reconciliation};
use term_document::{DiskCheck, FileContent, Outcome};
use term_ui::kit::alert::{Alert, Role};
use term_ui::workspace_host::Tabs;

use super::view::EditorTextView;
use super::{display_name, update_dirty};
use crate::util::{label, ns, set_action, symbol};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;

/// What the bar is about.
pub(crate) enum State {
    /// The file holds this version, which differs from the one the editor's text started from.
    Changed(FileContent),
    /// A merge marked this many places where both sides changed the same lines.
    Marked(usize),
    /// The file is gone.
    Removed,
}

/// The views of the bar, which takes no room while it is hidden.
pub(crate) struct Bar {
    view: Retained<NSBox>,
    message: Retained<NSTextField>,
    buttons: Retained<NSStackView>,
    collapsed: Retained<NSLayoutConstraint>,
}

impl Bar {
    pub(crate) fn new(mtm: MainThreadMarker) -> Bar {
        let view = NSBox::new(mtm);
        view.setBoxType(NSBoxType::Custom);
        view.setTitlePosition(NSTitlePosition::NoTitle);
        view.setBorderWidth(0.0);
        view.setCornerRadius(0.0);
        view.setFillColor(&NSColor::systemYellowColor().colorWithAlphaComponent(0.18));
        view.setContentViewMargins(NSSize::new(10.0, 5.0));
        let message = label(mtm, "");
        message.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        message
            .setContentCompressionResistancePriority_forOrientation(200.0, NSLayoutConstraintOrientation::Horizontal);
        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(term_ui::kit::style::space::S);
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setSpacing(term_ui::kit::style::space::M);
        row.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        if let Some(image) = symbol("exclamationmark.triangle.fill", "") {
            let icon = NSImageView::imageViewWithImage(&image, mtm);
            icon.setContentTintColor(Some(&NSColor::systemOrangeColor()));
            row.addView_inGravity(&icon, NSStackViewGravity::Leading);
        }
        row.addView_inGravity(&message, NSStackViewGravity::Leading);
        row.addView_inGravity(&buttons, NSStackViewGravity::Trailing);
        if let Some(content) = view.contentView() {
            content.addSubview(&row);
            crate::util::pin_edges(&row, &content);
        }
        let collapsed = view.heightAnchor().constraintEqualToConstant(0.0);
        collapsed.setActive(true);
        view.setHidden(true);
        Bar {
            view,
            message,
            buttons,
            collapsed,
        }
    }

    pub(crate) fn view(&self) -> &NSView {
        &self.view
    }

    fn show(&self, target: &EditorTextView, message: &str, buttons: &[(&str, Sel)]) {
        let changed = self.view.isHidden() || self.message.stringValue().to_string() != message;
        self.message.setStringValue(&ns(message));
        for old in self.buttons.arrangedSubviews().iter() {
            self.buttons.removeArrangedSubview(&old);
            old.removeFromSuperview();
        }
        for (title, action) in buttons {
            self.buttons.addArrangedSubview(&button(target, title, *action));
        }
        self.collapsed.setActive(false);
        self.view.setHidden(false);
        if changed {
            term_ui::kit::panel::announce(target.mtm(), message);
        }
    }

    fn hide(&self) {
        self.view.setHidden(true);
        self.collapsed.setActive(true);
    }
}

fn button(target: &EditorTextView, title: &str, action: Sel) -> Retained<NSButton> {
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, target.mtm()) };
    set_action(&button, target, action);
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Small);
    button.setFont(Some(&term_ui::kit::style::small_system()));
    button
}

/// Follows what a look at the file of `view` found. An editor without unsaved text takes a changed file as it is;
/// one with unsaved text gets the bar, whatever a merge would make of it, and a gone file gets it in any case.
pub(crate) fn follow(view: &EditorTextView, check: DiskCheck) {
    let content = match check {
        DiskCheck::Changed(content) => content,
        DiskCheck::Missing => {
            take(view, DiskCheck::Missing);
            set(view, Some(State::Removed));
            return;
        }
        check => {
            take(view, check);
            return;
        }
    };
    let asks = matches!(
        view.document().reconciliation(&content),
        Reconciliation::Merged(_) | Reconciliation::Conflict(_)
    );
    if asks {
        set(view, Some(State::Changed(content)));
        return;
    }
    take(view, DiskCheck::Changed(content));
    if matches!(
        *view.ivars().file.conflict.borrow(),
        Some(State::Changed(_) | State::Removed)
    ) {
        set(view, None);
    }
}

fn take(view: &EditorTextView, check: DiskCheck) {
    let outcome = view.document_mut().take_disk(check);
    match outcome {
        Ok(Outcome::Reloaded(changes) | Outcome::Merged(changes)) => {
            view.apply_model_changes(&changes);
            update_dirty(view);
        }
        Ok(Outcome::Conflict { disk, .. }) => set(view, Some(State::Changed(disk))),
        Ok(Outcome::Kept | Outcome::Removed) => update_dirty(view),
        Ok(Outcome::Unchanged) => {}
        Err(error) => term_base::diag::report("taking a file's change", format!("{error:?}")),
    }
}

fn set(view: &EditorTextView, state: Option<State>) {
    *view.ivars().file.conflict.borrow_mut() = state;
    refresh(view);
}

/// Shows the bar for the view's state, or hides it.
fn refresh(view: &EditorTextView) {
    let name = view.document().path().map(display_name).unwrap_or_default();
    let shown = match &*view.ivars().file.conflict.borrow() {
        None => None,
        Some(State::Changed(_)) => Some((
            tf("„{}“ wurde außerhalb geändert.", &[&name]),
            vec![
                (t("Unterschiede zeigen"), sel!(termConflictDiff:)),
                (t("Zusammenführen"), sel!(termConflictMerge:)),
                (t("Version von der Platte laden"), sel!(termConflictLoad:)),
                (t("Meine behalten"), sel!(termConflictKeep:)),
            ],
        )),
        Some(State::Marked(count)) => Some((
            if *count == 1 {
                tf("„{}“: ein Konflikt beim Zusammenführen", &[&name])
            } else {
                tf("„{}“: {} Konflikte beim Zusammenführen", &[&name, &count.to_string()])
            },
            vec![
                (t("Vorheriger Konflikt"), sel!(termConflictPrevious:)),
                (t("Nächster Konflikt"), sel!(termConflictNext:)),
            ],
        )),
        Some(State::Removed) => Some((
            tf("„{}“ wurde auf der Platte gelöscht.", &[&name]),
            vec![
                (t("Sichern"), sel!(termConflictSave:)),
                (t("Schließen"), sel!(termConflictClose:)),
            ],
        )),
    };
    let bar = view.ivars().around.bar.borrow();
    let Some(bar) = bar.as_ref() else { return };
    match shown {
        Some((message, buttons)) => bar.show(view, &message, &buttons),
        None => bar.hide(),
    }
}

/// The file's version the bar is about, taken out of the view's state; the state stays if it is about something else.
fn changed_version(view: &EditorTextView) -> Option<FileContent> {
    let mut state = view.ivars().file.conflict.borrow_mut();
    match state.take() {
        Some(State::Changed(content)) => Some(content),
        other => {
            *state = other;
            None
        }
    }
}

/// Takes `disk` as the file's version and makes the text `text`, as one undo step.
fn adopt(view: &EditorTextView, disk: FileContent, text: &str) {
    let adopted = view.document_mut().adopt_disk(disk, text);
    match adopted {
        Ok(changes) => {
            view.apply_model_changes(&changes);
            update_dirty(view);
        }
        Err(error) => term_base::diag::report("settling a change on disk", format!("{error:?}")),
    }
}

/// Opens a tab with the differences between the file's version and the editor's text.
pub(crate) fn show_differences(view: &EditorTextView) {
    let (Some(controller), Some(path)) = (view.controller(), view.document().path().map(ToOwned::to_owned)) else {
        return;
    };
    let state = view.ivars().file.conflict.borrow();
    let Some(State::Changed(disk)) = &*state else { return };
    let buffer = view.document().text().to_string();
    super::compare::show(&controller, &path, &disk.text, &buffer);
}

/// Merges the file's changes into the editor's text against the version both started from: a clean merge as one undo
/// step, overlapping changes between conflict markers for the user to settle.
pub(crate) fn merge(view: &EditorTextView) {
    let Some(disk) = changed_version(view) else { return };
    let reconciliation = view.document().reconciliation(&disk);
    match reconciliation {
        Reconciliation::Merged(text) => {
            adopt(view, disk, &text);
            set(view, None);
        }
        Reconciliation::Conflict(conflict) => {
            let marked = conflict.with_markers(t("deine Fassung"), t("Fassung auf der Platte"));
            adopt(view, disk, &marked);
            set(view, Some(State::Marked(conflict.hunks().len())));
            go_to_conflict(view, true);
        }
        Reconciliation::KeepBuffer => {
            let text = view.document().text().to_string();
            adopt(view, disk, &text);
            set(view, None);
        }
        Reconciliation::Reload => {
            let text = disk.text.clone();
            adopt(view, disk, &text);
            set(view, None);
        }
    }
}

/// Makes the editor's text the file's version, as one undo step.
pub(crate) fn load(view: &EditorTextView) {
    let Some(disk) = changed_version(view) else { return };
    let text = disk.text.clone();
    adopt(view, disk, &text);
    set(view, None);
}

/// Keeps the editor's text, unsaved, against the file's version.
pub(crate) fn keep(view: &EditorTextView) {
    let Some(disk) = changed_version(view) else { return };
    let text = view.document().text().to_string();
    adopt(view, disk, &text);
    set(view, None);
}

/// Writes the text of a gone file anew.
pub(crate) fn save(view: &EditorTextView) {
    super::save_soon(view, false);
}

/// Closes the editor of a gone file once the user agreed.
pub(crate) fn close(view: &EditorTextView) {
    if let (Some(controller), Some(tab)) = (view.controller(), view.ivars().tab.get()) {
        controller.close_tab(tab);
    }
}

/// Whether the file of `view` is gone from the disk.
pub(crate) fn is_gone(view: &EditorTextView) -> bool {
    matches!(*view.ivars().file.conflict.borrow(), Some(State::Removed))
}

/// Whether the editor of a gone file may close, asked every time, saved or not, since its text may be the file's last
/// copy: the user let the text go, or saved it, after which `again` hears whether it was written.
pub(crate) fn may_close(view: &EditorTextView, again: &super::closing::Again) -> super::closing::Answer {
    let answer = discard_question(view).run_modal();
    if answer == NSAlertFirstButtonReturn {
        super::closing::save_for(view, again)
    } else if answer == NSAlertSecondButtonReturn {
        super::closing::give_up(view);
        super::closing::Answer::Yes
    } else {
        super::closing::Answer::No
    }
}

/// The question before the text of a gone file's editor goes; Return cancels, as with every loss that cannot be
/// undone.
fn discard_question(view: &EditorTextView) -> Alert {
    let name = view.document().path().map(display_name).unwrap_or_default();
    let (message, informative) = if view.document().has_own_changes() {
        (
            tf("Ungesicherte Änderungen an „{}“ verwerfen?", &[&name]),
            t("Ohne Sichern gehen deine Änderungen verloren."),
        )
    } else {
        (
            tf("„{}“ wurde gelöscht. Inhalt verwerfen?", &[&name]),
            t("Dieser Editor hält die letzte Kopie der Datei. „Sichern“ stellt sie wieder her."),
        )
    };
    Alert::new(view.mtm(), &message)
        .warning()
        .informative(informative)
        .button(t("Sichern"), Role::Plain)
        .button(t("Verwerfen"), Role::Destructive)
        .button(t("Abbrechen"), Role::CancelOnReturn)
}

/// The file now holds the editor's text: the bar about its change or its loss goes.
pub(crate) fn saved(view: &EditorTextView) {
    if matches!(
        *view.ivars().file.conflict.borrow(),
        Some(State::Changed(_) | State::Removed)
    ) {
        set(view, None);
    }
}

/// The text changed: while conflict markers are in it, they are counted again once typing rests.
pub(crate) fn text_changed(view: &EditorTextView) {
    if matches!(*view.ivars().file.conflict.borrow(), Some(State::Marked(_))) {
        view.later(sel!(termConflictRecount), 0.3);
    }
}

/// Counts the conflict markers left; the bar goes with the last.
pub(crate) fn recount(view: &EditorTextView) {
    if !matches!(*view.ivars().file.conflict.borrow(), Some(State::Marked(_))) {
        return;
    }
    let count = merge::conflict_markers(&view.document().text().to_string()).len();
    set(view, (count > 0).then_some(State::Marked(count)));
}

/// Selects the next conflict marker after the selection, or the previous one before it, wrapping around.
pub(crate) fn go_to_conflict(view: &EditorTextView, forward: bool) {
    let range = {
        let document = view.document();
        let text = document.text();
        let marks = merge::conflict_markers(&text.to_string());
        let caret = text.utf16_to_byte(view.selectedRange().location).unwrap_or(0);
        let found = if forward {
            marks.iter().find(|&&at| at > caret).or_else(|| marks.first())
        } else {
            marks.iter().rev().find(|&&at| at < caret).or_else(|| marks.last())
        };
        let Some(&at) = found else { return };
        let line = text.line_of(at);
        let (Some(bytes), Ok(start)) = (text.line_content(line), text.byte_to_utf16(at)) else {
            return;
        };
        let end = text.byte_to_utf16(bytes.end).unwrap_or(start);
        NSRange::new(start, end - start)
    };
    view.setSelectedRange(range);
    view.scrollRangeToVisible(range);
    if let Some(window) = view.window() {
        window.makeFirstResponder(Some(view));
    }
}
