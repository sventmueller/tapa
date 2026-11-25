//! What the self-test reads from and does with the activity popover, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// The lines of the open popover, each with its buttons; none while it is closed.
pub fn activity_lines() -> Option<Vec<String>> {
    let activity = of().filter(|a| a.is_shown())?;
    let (key, part) = activity.ivars().only.get()?;
    let lines = session_lines(key, part, SystemTime::now())
        .into_iter()
        .map(|l| {
            let buttons: Vec<&str> = l.buttons.iter().map(|(title, _)| *title).collect();
            format!("{} — {} [{}]", l.title, l.detail, buttons.join(", "))
        })
        .collect();
    Some(lines)
}

/// Clicks the first button of the open popover titled `title`; whether there was one.
pub fn press_activity(title: &str) -> bool {
    let Some(activity) = of().filter(|a| a.is_shown()) else {
        return false;
    };
    let button = activity.ivars().shown.borrow().as_ref().and_then(|shown| {
        shown
            .lines
            .iter()
            .flat_map(|line| &line.buttons)
            .find(|(_, shown)| *shown == title)
            .map(|(button, _)| button.clone())
    });
    let Some(button) = button else { return false };
    // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
    unsafe { button.performClick(None) };
    true
}

/// The content of the open popover, for a picture.
pub fn activity_view() -> Option<Retained<NSView>> {
    let activity = of().filter(|a| a.is_shown())?;
    let popover = activity.ivars().popover.borrow().clone()?;
    Some(popover.contentViewController()?.view())
}

/// How often a popover changed the texts of its rows in place.
pub(super) static UPDATED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How often a popover changed the texts of its rows in place, for the self-test.
pub fn activity_updated_in_place() -> usize {
    UPDATED.load(std::sync::atomic::Ordering::Relaxed)
}
