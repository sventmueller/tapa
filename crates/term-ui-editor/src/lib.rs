//! The editor tab of a workspace (E22): a TextKit 2 text view over one document of the model in `term-document`, with
//! syntax colours, folds, further carets, Vim's keys (E105), change marks against the last commit, the bar about a
//! file changed on disk, and Markdown's format commands with "In Splat öffnen".
//!
//! It starts no formatter and no language server (E16); its copies of unsaved text against a crash are sealed with
//! the key the app hands over (E17). The app installs a [`Host`]; the editor reaches the rest of its workspace through
//! `term_ui::workspace_host`.

mod changes;
mod closing;
mod compare;
mod conflict;
mod container;
mod context_menu;
mod editing;
mod file;
mod fold;
mod highlight;
mod host;
mod indent;
mod insets;
mod long_lines;
mod markdown;
mod multi;
mod on_disk;
mod opening;
mod overlay;
mod place;
mod reading;
mod recovery;
mod resync;
mod ruler;
mod saving;
mod settings_change;
mod status;
mod style;
mod text_view;
mod util;
mod view;
mod viewport;
mod vim;
mod whitespace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSTextView, NSView};
use objc2_foundation::NSRect;
use term_document::Change;
use term_document::recovery::RecoveryId;
use term_ui::workspace_host::{AsWorkspace, TabId, Workspace, WorkspaceId};

pub use closing::{Again, Answer};
pub use host::{Host, Settings, install};
pub use markdown::{format_menu, open_in_splat_item, open_in_splat_menu_item, paste_plain_item, preview_item};
pub use recovery::QuitCopies;
pub use text_view::EditorSerial;

pub(crate) use container::redraw_gutter;
pub(crate) use file::update_dirty;
pub(crate) use opening::toggle_wrap;
#[cfg(feature = "selftest")]
pub(crate) use saving::run_until;
pub(crate) use saving::{save_soon, save_then};
pub(crate) use style::code_font;
pub(crate) use util::undo_step;
pub(crate) use view::EditorTextView;

/// The editors' state beside the views: the list of them and what they share.
#[derive(Default)]
pub(crate) struct EditorState {
    editors: RefCell<Vec<Weak<EditorTextView>>>,
    vim: vim::Enabled,
    comparisons: compare::Comparisons,
    recovery_copies: recovery::RecoveryCopies,
    reads: reading::Reads,
    /// Each workspace's editor tab that the next file opened as a preview replaces, with its key.
    previews: RefCell<HashMap<WorkspaceId, (TabId, String)>>,
}

thread_local! {
    static STATE: Rc<EditorState> = Rc::new(EditorState::default());
}

pub(crate) fn state() -> Rc<EditorState> {
    STATE.with(Rc::clone)
}

/// The editors still open.
fn open_editors() -> Vec<Retained<EditorTextView>> {
    let state = state();
    let mut editors = state.editors.borrow_mut();
    editors.retain(|view| view.load().is_some());
    editors.iter().filter_map(Weak::load).collect()
}

/// The open editor with `serial`, the number that other threads hold instead of the view.
pub(crate) fn find_editor(serial: EditorSerial) -> Option<Retained<EditorTextView>> {
    open_editors().into_iter().find(|v| v.ivars().serial == serial)
}

/// The UTF-16 range of the lines on screen, with a margin.
pub(crate) fn visible_utf16(view: &EditorTextView) -> std::ops::Range<usize> {
    utf16_in(view, view.visibleRect())
}

/// The UTF-16 range of the lines laid out in `rect` of the view, with a margin of lines on both sides.
pub(crate) fn utf16_in(view: &EditorTextView, rect: NSRect) -> std::ops::Range<usize> {
    let lines = highlight::lines_in(view, rect);
    let length = view.document().text().len_utf16();
    let start = view.line_start(lines.start).unwrap_or(0);
    let end = view.line_start(lines.end).unwrap_or(length);
    start..end.max(start)
}

/// Adds the controls of the editor's own, Vim's mode, the indentation and dictation, to its status bar's `stack`.
pub(crate) fn status_items(mtm: MainThreadMarker, view: &EditorTextView, stack: &objc2_app_kit::NSStackView) {
    vim::status_items(mtm, view, stack);
    indent::status_items(mtm, view, stack);
    #[cfg(feature = "dictation")]
    term_ui_dictation::editor_items(mtm, std::rc::Rc::new(EditorField(view.ivars().workspace)), view, stack);
}

/// The editor of a workspace as a field that dictation goes into.
#[cfg(feature = "dictation")]
struct EditorField(term_ui::workspace_host::WorkspaceId);

#[cfg(feature = "dictation")]
impl term_ui_dictation::DictationField for EditorField {
    fn notice(&self, text: &str) -> bool {
        use term_ui::workspace_host::Notices;
        term_ui::workspace_host::Workspace::new(self.0).show_notice(text);
        true
    }
}

/// The name of `path` as a tab and an alert show it.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// What other parts of the app follow of an edit.
fn notify_change(view: &EditorTextView, _change: &Change) {
    host::edited(view);
}

/// The editors of `workspace`.
fn editors_in(workspace: &Workspace) -> Vec<Retained<EditorTextView>> {
    open_editors()
        .into_iter()
        .filter(|view| view.ivars().workspace == workspace.id())
        .collect()
}

/// The editor of the document `tab` in `workspace`.
fn editor_of_tab(workspace: &Workspace, tab: TabId) -> Option<Retained<EditorTextView>> {
    editors_in(workspace)
        .into_iter()
        .find(|view| view.ivars().tab.get() == Some(tab))
}

/// The key of the editor tab of `path`: its canonical path, so that a file opens in one tab whatever path led to it.
pub fn tab_key(path: &Path) -> String {
    canonical(path).to_string_lossy().into_owned()
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Opens `path` in an editor tab of `workspace`, or brings its open tab to the front; with `preview` in the
/// workspace's one preview tab, which the next preview replaces until the user edits or keeps it. A file that cannot
/// be read shows an alert.
pub fn open_tab(workspace: &(impl AsWorkspace + ?Sized), path: &Path, preview: bool) {
    let workspace = workspace.as_workspace();
    opening::open_tab(&workspace, &canonical(path), preview);
}

/// Keeps the preview tab of `workspace` as a tab of its own, as a double click on its file does.
pub fn keep_preview(workspace: &(impl AsWorkspace + ?Sized)) {
    state().previews.borrow_mut().remove(&workspace.as_workspace().id());
}

/// Opens an empty, untitled editor tab in `workspace`.
pub fn new_untitled(workspace: &(impl AsWorkspace + ?Sized)) {
    opening::new_untitled(&workspace.as_workspace());
}

/// Opens `path` in `workspace`, or selects its tab, and puts the cursor on line `line` at column `column`, both
/// counted from 1 in characters as compilers print them.
pub fn reveal_position(workspace: &(impl AsWorkspace + ?Sized), path: &Path, line: usize, column: usize) {
    opening::reveal_position(&workspace.as_workspace(), &canonical(path), line, column);
}

/// Opens `path` in `workspace`, or selects its tab, and marks `columns`, in UTF-16 code units, of line `line`,
/// counted from 1; `focus` selects them and gives the editor the keyboard focus.
pub fn reveal(
    workspace: &(impl AsWorkspace + ?Sized),
    path: &Path,
    line: u64,
    columns: std::ops::Range<usize>,
    focus: bool,
) {
    opening::reveal(&workspace.as_workspace(), &canonical(path), line, columns, focus);
}

/// The copies of unsaved text a crash left behind, which [`open_recovered`] brings back; none without a key.
pub fn pending_copies() -> Vec<term_document::recovery::Pending> {
    let Some(copies) = recovery::copies() else {
        return Vec::new();
    };
    copies.pending().unwrap_or_else(|error| {
        term_base::diag::report("recovery copies", error);
        Vec::new()
    })
}

/// Opens the copy `id` a crash left behind in a new editor tab of `workspace`, marked unsaved, then looks at its file:
/// one unchanged since keeps the text as it came back, one changed meanwhile gets the bar above the editor.
pub fn open_recovered(workspace: &(impl AsWorkspace + ?Sized), id: &RecoveryId) -> Option<EditorSerial> {
    let copies = recovery::copies()?;
    let recovered = copies
        .restore(id)
        .map_err(|error| term_base::diag::report("recovery copy", error))
        .ok()?;
    let font_size = host::settings().font_size;
    let view = opening::open_recovered(&workspace.as_workspace(), recovered, font_size);
    on_disk::look(&view, on_disk::Look::Recovered);
    Some(view.ivars().serial)
}

/// The byte offset where the first line on screen of `editor` starts, which a preview beside it follows.
pub fn top_byte(editor: EditorSerial) -> Option<usize> {
    place::top_byte(&*find_editor(editor)?)
}

/// The top of the line of `editor` holding byte `byte`, in the text view's coordinates, after TextKit laid out the
/// whole text; `near` asks only for a line beside those on screen, which needs no such layout.
pub fn byte_y(editor: EditorSerial, byte: usize, near: bool) -> Option<f64> {
    let view = find_editor(editor)?;
    if near {
        place::byte_y_near(&view, byte)
    } else {
        place::byte_y(&view, byte)
    }
}

/// The text of `editor` as it is now, a cheap copy that a preview lays out off the main thread.
pub fn text_snapshot(editor: EditorSerial) -> Option<term_document::Text> {
    Some(find_editor(editor)?.document().text().clone())
}

/// Shows `beside`, such as a Markdown preview, to the right of the text of `editor`, or with `None` takes it away; the
/// format bar's preview button follows [`Host::preview_shown`].
pub fn set_beside(editor: EditorSerial, beside: Option<&NSView>) {
    if let Some(view) = find_editor(editor) {
        container::set_beside(&view, beside);
        markdown::refresh_bar(&view);
    }
}

/// The text view of `editor`, as a preview beside it scrolls it.
pub fn text_view(editor: EditorSerial) -> Option<Retained<NSTextView>> {
    find_editor(editor).map(Retained::into_super)
}

/// Fills `host`, the view of a tab the app restored, with an editor of `path`; the text a crash left unsaved comes
/// back with [`open_recovered`] instead. `None` when the file cannot be read.
pub fn attach_restored(
    workspace: &(impl AsWorkspace + ?Sized),
    tab: TabId,
    host: &NSView,
    path: &Path,
) -> Option<EditorSerial> {
    let workspace = workspace.as_workspace();
    let document = term_document::Document::open(path).ok()?;
    let view = opening::attach_restored(&workspace, tab, host, document, RecoveryId::new());
    Some(view.ivars().serial)
}

/// The editor in `tab` of `workspace` is about to close.
pub fn closing(workspace: &(impl AsWorkspace + ?Sized), tab: TabId) {
    let workspace = workspace.as_workspace();
    if let Some(view) = editor_of_tab(&workspace, tab) {
        if let Some(host) = host::installed() {
            host.closing(view.ivars().serial);
        }
        state()
            .editors
            .borrow_mut()
            .retain(|w| w.load().is_some_and(|v| !std::ptr::eq(&*v, &*view)));
    }
}

/// Whether the editor in `tab` of `workspace` may close: saved, or the user said so; `Later` when the user asked to
/// save, and `again` hears once the write is over. `Yes` for a tab that is no editor.
pub fn may_close(workspace: &(impl AsWorkspace + ?Sized), tab: TabId, again: &Again) -> Answer {
    closing::may_close(&workspace.as_workspace(), tab, again)
}

/// Whether the app may quit, asking about unsaved text once for all editors; `Later` while saves run.
pub fn confirm_quit(mtm: MainThreadMarker, again: &Again) -> Answer {
    closing::confirm_quit(mtm, again)
}

/// At a quit the user agreed to: drops the copies of every open editor; what to wait for before the app ends.
pub fn quit() -> QuitCopies {
    recovery::quit()
}

/// The names of the documents in `workspace` whose text closing it would lose.
pub fn unsaved_in(workspace: &(impl AsWorkspace + ?Sized)) -> Vec<String> {
    closing::unsaved_in(&workspace.as_workspace())
}

/// Gives up the changes in the editors of `workspace` and their copies, as "Nicht sichern" does.
pub fn discard_changes(workspace: &(impl AsWorkspace + ?Sized)) {
    closing::discard_changes(&workspace.as_workspace());
}

/// Writes the copy of every editor of `workspace` that holds unsaved text, as after a close the user cancelled.
pub fn store_copies(workspace: &(impl AsWorkspace + ?Sized)) {
    closing::store_copies(&workspace.as_workspace());
}

/// The files below `root` whose open editors have unsaved changes.
pub fn unsaved_below(root: &Path) -> Vec<PathBuf> {
    closing::unsaved_below(root)
}

/// Saves the editors of files below `root` with unsaved changes and calls `then` once the writes are over, with
/// whether none is left unsaved.
pub fn save_below(root: &Path, then: Box<dyn FnOnce(bool)>) {
    closing::save_below(root, then);
}

/// The open editors below `root` take what changed on disk, after a git operation of the app's own.
pub fn check_disk_below(root: &Path) {
    for view in open_editors() {
        if view.document().path().is_some_and(|path| path.starts_with(root)) {
            on_disk::look(&view, on_disk::Look::Changed);
        }
    }
}

/// Saves every editor with unsaved changes, as "Alles sichern" does, and calls `then` once the writes are over, with
/// whether none is left unsaved; an untitled one asks for its file.
pub fn save_all(then: Box<dyn FnOnce(bool)>) {
    closing::save_every(then);
}

/// Whether an open editor has unsaved changes, as "Alles sichern" is enabled then.
pub fn has_unsaved() -> bool {
    closing::dirty_count() > 0
}

/// Every open editor takes what changed on disk, as when the app comes to the front again and no watcher saw it.
pub fn check_disk() {
    for view in open_editors() {
        if view.document().path().is_some() {
            on_disk::look(&view, on_disk::Look::Changed);
        }
    }
}

/// Looks at the files of open editors in `dirs` and takes what changed on disk, as the watcher of the file list
/// reports it.
pub fn files_changed(dirs: &[PathBuf]) {
    for view in open_editors() {
        let path = view.document().path().map(Path::to_path_buf);
        let Some(path) = path else { continue };
        if !path.parent().is_some_and(|parent| dirs.iter().any(|dir| dir == parent)) {
            continue;
        }
        on_disk::look(&view, on_disk::Look::Changed);
    }
}

/// Something changed in a `.git` folder below `root`: the change marks of its editors follow the commit.
pub fn git_changed(root: &Path) {
    changes::git_changed(root);
}

/// The user's settings changed: the editors read them again and show them.
pub fn settings_changed() {
    settings_change::apply();
}

pub use settings_change::{FONT_MAX, FONT_MIN};

/// Whether `responder`, the view with the keyboard, is in an editor: its text or its find bar.
pub fn holds_keyboard(responder: &NSView) -> bool {
    settings_change::holds(responder)
}

/// Shows the text of the editor that `responder` is in `change` points larger, or at the settings' size again for a
/// change of 0, until the settings change; the settings stay as they are.
pub fn zoom(responder: &NSView, change: f64) {
    settings_change::zoom(responder, change);
}

/// The text view of the editor in `tab` of `workspace`, as the window makes it the first responder.
pub fn text_view_of_tab(workspace: &(impl AsWorkspace + ?Sized), tab: TabId) -> Option<Retained<NSTextView>> {
    editor_of_tab(&workspace.as_workspace(), tab).map(Retained::into_super)
}

/// The file and the editor in `tab` of `workspace`, if that tab is an editor, as the app keeps it for restoring.
pub fn tab_document(workspace: &(impl AsWorkspace + ?Sized), tab: TabId) -> Option<(Option<PathBuf>, EditorSerial)> {
    let view = editor_of_tab(&workspace.as_workspace(), tab)?;
    let path = view.document().path().map(Path::to_path_buf);
    Some((path, view.ivars().serial))
}

/// The file and the text of the editor `editor`, as a preview beside it shows them.
pub fn text_of(editor: EditorSerial) -> Option<(Option<PathBuf>, String)> {
    let view = find_editor(editor)?;
    let document = view.document();
    Some((document.path().map(Path::to_path_buf), document.text().to_string()))
}

/// The editor of `workspace` that has the keyboard focus, else the one in view.
pub fn focused_editor(workspace: &(impl AsWorkspace + ?Sized)) -> Option<EditorSerial> {
    let workspace = workspace.as_workspace();
    let mine = editors_in(&workspace);
    let responder = workspace.window().and_then(|w| w.firstResponder());
    mine.iter()
        .find(|e| {
            responder
                .as_ref()
                .is_some_and(|r| std::ptr::eq(Retained::as_ptr(r).cast::<u8>(), Retained::as_ptr(e).cast::<u8>()))
        })
        .or_else(|| mine.iter().find(|e| !e.isHiddenOrHasHiddenAncestor()))
        .map(|view| view.ivars().serial)
}

/// What the self-test reads from and does with editors, through the code their commands run.
#[cfg(feature = "selftest")]
pub mod selftest;
