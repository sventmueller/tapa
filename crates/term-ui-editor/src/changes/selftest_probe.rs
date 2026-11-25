//! What the self-test reads from and does with the parts of `changes.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// The hunks of `view` as kinds and line ranges, and the text of the popover's hunk, for the self-test.
/// What the self-test reads of an editor's change marks.
pub(crate) struct Seen {
    /// The file's last commit is known.
    pub(crate) head: bool,
    /// Each hunk's mark, with removals as `RemovedAbove`, and its lines.
    pub(crate) hunks: Vec<(Mark, Range<usize>)>,
    /// The committed lines the popover shows, while it is shown.
    pub(crate) popover: Option<String>,
}

pub(crate) fn seen(view: &EditorTextView) -> Seen {
    let marks = view.ivars().marks.borrow();
    let hunks = marks
        .hunks
        .iter()
        .map(|h| {
            let mark = match h.change.kind {
                ChangeKind::Added => Mark::Added,
                ChangeKind::Modified => Mark::Modified,
                ChangeKind::Removed => Mark::RemovedAbove,
            };
            (mark, h.change.lines.clone())
        })
        .collect();
    let popover = marks
        .popover
        .as_ref()
        .filter(|p| p.isShown())
        .and(marks.shown.as_ref())
        .zip(marks.head.as_ref())
        .map(|((hunk, _), head)| committed_lines(&head.text, &hunk.old_lines));
    Seen {
        head: marks.head.is_some(),
        hunks,
        popover,
    }
}

/// The popover's window, for a picture, and its button, for a click.
pub(crate) fn popover_parts(
    view: &EditorTextView,
) -> Option<(Retained<objc2_app_kit::NSWindow>, Retained<objc2_app_kit::NSButton>)> {
    let marks = view.ivars().marks.borrow();
    let content = marks.popover.as_ref()?.contentViewController()?.view();
    let stack = content.subviews().firstObject()?.downcast::<NSStackView>().ok()?;
    let button = stack
        .arrangedSubviews()
        .iter()
        .find_map(|v| v.downcast::<objc2_app_kit::NSButton>().ok())?;
    Some((stack.window()?, button))
}
