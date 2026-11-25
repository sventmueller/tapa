//! What the user can hand the agent besides files: the output of the terminal in view and the selection of an
//! editor. Each becomes an attachment of the message; the terminal's is outside text.

use term_thread::Attachment;
use term_ui::workspace_host::AsWorkspace;

/// How many lines of terminal output go along at most, the latest.
const TERMINAL_LINES: usize = 200;

/// The output of the terminal in view, else of the one opened last.
pub(crate) fn terminal(controller: &(impl AsWorkspace + ?Sized)) -> Option<Attachment> {
    let controller = &controller.as_workspace();
    let (label, text) = crate::host::host().recent_output(controller.id())?;
    let lines: Vec<&str> = text.lines().collect();
    let shown = &lines[lines.len().saturating_sub(TERMINAL_LINES)..];
    Some(super::outside::terminal(&label, shown.join("\n")))
}

/// The selection of the editor in view in `controller`'s window, else of any of its editors that has one.
pub(crate) fn editor_selection(controller: &(impl AsWorkspace + ?Sized)) -> Option<Attachment> {
    let controller = &controller.as_workspace();
    let (path, lines, text) = crate::host::host().editor_selection(controller.id())?;
    Some(Attachment::Selection { path, lines, text })
}
