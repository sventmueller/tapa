//! What the self-test reads from and does with the parts of `conflict.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// The message and informative text of the question before the text of `view` goes, and its buttons, each with the
/// key that presses it.
pub(crate) fn inspect_question(view: &EditorTextView) -> (String, String, Vec<(String, String)>) {
    let question = discard_question(view);
    let alert = question.ns();
    let buttons = alert
        .buttons()
        .iter()
        .map(|button| (button.title().to_string(), button.keyEquivalent().to_string()))
        .collect();
    (
        alert.messageText().to_string(),
        alert.informativeText().to_string(),
        buttons,
    )
}

/// What the bar says and offers, for the self-test.
pub(crate) fn inspect(view: &EditorTextView) -> Option<(String, Vec<String>)> {
    let bar = view.ivars().around.bar.borrow();
    let bar = bar.as_ref()?;
    if bar.view.isHidden() {
        return None;
    }
    let titles = bar
        .buttons
        .arrangedSubviews()
        .iter()
        .filter_map(|view| view.downcast::<NSButton>().ok())
        .map(|button| button.title().to_string())
        .collect();
    Some((bar.message.stringValue().to_string(), titles))
}

/// Clicks the bar's button titled `title`, for the self-test; whether there was one.
pub(crate) fn press(view: &EditorTextView, title: &str) -> bool {
    let button = {
        let bar = view.ivars().around.bar.borrow();
        let Some(bar) = bar.as_ref() else { return false };
        bar.buttons
            .arrangedSubviews()
            .iter()
            .filter_map(|view| view.downcast::<NSButton>().ok())
            .find(|button| button.title().to_string() == title)
    };
    button.is_some_and(|button| {
        // SAFETY: A click on a button of the bar, whose target is the editor, as a person's click would be.
        unsafe { button.performClick(None) };
        true
    })
}
