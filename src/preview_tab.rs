//! Documents as they read in the app (E22): a Markdown editor's preview beside its text, and tabs for pictures, PDF
//! files and what Quick Look shows; `term-ui-preview` builds them, the hosts of the editor and the file list call in
//! here.

use std::path::Path;

use objc2::MainThreadMarker;
use term_ui::workspace_host::WorkspaceId;
use term_ui_editor::EditorSerial;

use crate::state::app;

/// Tells the render path where its page lies and the previews how large the editors' text is; once, before the first
/// window opens.
pub(crate) fn start(mtm: MainThreadMarker) {
    term_ui_preview::start(mtm);
    settings_changed();
}

/// The settings changed: the previews follow the editors' text size.
pub(crate) fn settings_changed() {
    term_ui_preview::font_size_changed(app().config().editor.font_size);
}

/// The user opened `path` in `editor`; a preview opens only when asked for.
pub(crate) fn opened(_workspace: WorkspaceId, _editor: EditorSerial, _path: &Path) {}

/// The text of `editor` changed, which its preview follows.
pub(crate) fn edited(editor: EditorSerial, _path: Option<&Path>) {
    term_ui_preview::edited(editor);
}

/// "Vorschau daneben ein-/ausblenden" in the Markdown document of `editor`.
pub(crate) fn toggle(workspace: WorkspaceId, editor: EditorSerial) {
    term_ui_preview::toggle_beside(workspace, editor);
}

/// Whether a preview shows beside `editor`.
pub(crate) fn shown(editor: EditorSerial) -> bool {
    term_ui_preview::is_beside(editor)
}

/// The editor `editor` is about to close, and its preview with it.
pub(crate) fn closing(editor: EditorSerial) {
    term_ui_preview::closing(editor);
}

/// "Als Vorschau öffnen": the drawing of the SVG file `path`, decoded by the sandboxed service.
pub(crate) fn open_as_preview(workspace: WorkspaceId, path: &Path) {
    term_ui_preview::open_drawing(workspace, path);
}

/// Opens `path` in a tab that shows it when it is a picture, a PDF or what Quick Look shows; `false` for a file an
/// editor should take.
pub(crate) fn open_document(workspace: WorkspaceId, path: &Path) -> bool {
    term_ui_preview::open_document(workspace, path)
}
