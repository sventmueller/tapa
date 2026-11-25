//! The editor tabs in the app (E22): `term-ui-editor` adds them to a workspace through `term_ui::workspace_host`, and
//! the app is that crate's `Host` for the settings, the key that seals its copies of unsaved text (E17) and the
//! unsaved mark of a tab. Pictures, PDF files and what Quick Look shows open in the preview's tabs instead.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use block2::RcBlock;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSView};
use objc2_foundation::NSURL;
use splat_i18n::t;
use term_base::guarded;
use term_ui::kit::ns;
use term_ui::workspace_host::{TabId, TabKind, Workspace as Handle, WorkspaceId};
use term_ui_editor::{EditorSerial, Settings};

use crate::state::{app, try_app};
use crate::tab::Tab;
use crate::window::Window;

/// The app as the editors' host.
struct EditorHost;

static HOST: EditorHost = EditorHost;

impl term_ui_editor::Host for EditorHost {
    fn settings(&self) -> Settings {
        let editor = app().config().editor.clone();
        Settings {
            font_size: editor.font_size,
            vim_mode: editor.vim,
            vim_clipboard: editor.vim_clipboard,
            show_whitespace: editor.show_whitespace,
            wrap: editor.wrap,
        }
    }

    fn set_vim_mode(&self, on: bool) {
        write(&["editor", "vim"], on);
    }

    fn set_show_whitespace(&self, on: bool) {
        write(&["editor", "show_whitespace"], on);
    }

    fn recovery_key(&self) -> Option<[u8; term_seal::KEY_LEN]> {
        crate::restore::seal_key()
            .map_err(|error| term_base::diag::report("the key of the editors' copies", error))
            .ok()
    }

    fn set_tab_dirty(&self, workspace: WorkspaceId, tab: TabId, dirty: bool) {
        if let Some((window, _)) = find(workspace) {
            window.set_hosted_dirty(workspace.get(), tab.get(), dirty);
        }
    }

    fn saved(&self, workspace: WorkspaceId, path: &Path) {
        if let Some((_, workspace)) = find(workspace)
            && path.starts_with(&workspace.base)
        {
            term_ui_git::changed(&workspace.base);
        }
    }

    fn opened(&self, workspace: WorkspaceId, editor: EditorSerial, path: &Path) {
        crate::preview_tab::opened(workspace, editor, path);
    }

    fn edited(&self, editor: EditorSerial, path: Option<&Path>) {
        crate::preview_tab::edited(editor, path);
    }

    fn toggle_preview(&self, workspace: WorkspaceId, editor: EditorSerial) {
        crate::preview_tab::toggle(workspace, editor);
    }

    fn preview_shown(&self, editor: EditorSerial) -> bool {
        crate::preview_tab::shown(editor)
    }

    fn closing(&self, editor: EditorSerial) {
        crate::preview_tab::closing(editor);
    }
}

/// Writes the switch at `path` of the settings file, which the editors then read again.
fn write(path: &[&str], on: bool) {
    let written = crate::settings::apply(|doc| {
        doc.set(path, on);
        Ok(true)
    });
    if let Err(error) = written {
        term_base::diag::report("an editor setting", error);
    }
}

/// Hands the editors their host; once, before the first window opens.
pub(crate) fn install() {
    term_ui_editor::install(&HOST);
}

/// The window and the workspace whose id is `workspace`, while it is open.
fn find(workspace: WorkspaceId) -> Option<(Rc<Window>, Rc<crate::workspace::Workspace>)> {
    let app = try_app()?;
    let windows = app.windows.borrow();
    windows.iter().find_map(|window| {
        let found = window.workspaces().into_iter().find(|own| own.id == workspace.get())?;
        Some((window.clone(), found))
    })
}

/// The handle the UI crates know the window's selected workspace by, unless it is the quick terminal's.
fn selected(window: &Window) -> Option<Handle> {
    if window.is_quick() {
        return None;
    }
    let workspace = window.selected_workspace()?;
    Some(Handle::new(WorkspaceId::new(workspace.id)))
}

/// Opens `path` in workspace `workspace`: a picture, a PDF or what Quick Look shows in a preview tab, every other file
/// in an editor tab, the preview tab with `preview`.
pub(crate) fn open_in(workspace: WorkspaceId, path: &Path, preview: bool) {
    if crate::preview_tab::open_document(workspace, path) {
        return;
    }
    term_ui_editor::open_tab(&Handle::new(workspace), path, preview);
}

/// Opens `path` in the window's selected workspace.
#[cfg(feature = "selftest")]
pub(crate) fn open(window: &Window, path: &Path) {
    match selected(window) {
        Some(workspace) => open_in(workspace.id(), path, false),
        None => objc2_app_kit::NSBeep(),
    }
}

/// "Öffnen …": asks in a sheet for files, starting in the selected workspace's base folder, and opens them there.
pub(crate) fn ask_and_open(window: &Window) {
    let (Some(workspace), Some(ns_window)) = (selected(window), window.front_ns_window()) else {
        return objc2_app_kit::NSBeep();
    };
    let panel = NSOpenPanel::openPanel(app().mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(true);
    panel.setPrompt(Some(&ns(t("Öffnen"))));
    if let Some(base) = workspace.project_root() {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&ns(&base.to_string_lossy()))));
    }
    let chosen = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        guarded((), || {
            if response != NSModalResponseOK {
                return;
            }
            let paths: Vec<PathBuf> = chosen
                .URLs()
                .iter()
                .filter_map(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
                .collect();
            for path in paths {
                open_in(workspace.id(), &path, false);
            }
        });
    });
    panel.beginSheetModalForWindow_completionHandler(&ns_window, &handler);
}

/// "Neue Datei": an empty, untitled editor tab in the window's selected workspace.
pub(crate) fn new_file(window: &Window) {
    match selected(window) {
        Some(workspace) => term_ui_editor::new_untitled(&workspace),
        None => objc2_app_kit::NSBeep(),
    }
}

/// "Alles sichern": saves every editor with unsaved changes.
pub(crate) fn save_all() {
    term_ui_editor::save_all(Box::new(|_| ()));
}

/// The file the editor tab `tab` of workspace `workspace` shows, which the restore keeps; none for another tab and an
/// untitled one.
pub(crate) fn kept(workspace: u64, tab: &Tab) -> Option<PathBuf> {
    tab.as_hosted().filter(|hosted| hosted.kind() == TabKind::File)?;
    let (path, _) = term_ui_editor::tab_document(&Handle::new(WorkspaceId::new(workspace)), TabId::new(tab.id))?;
    path
}

/// Opens the editor tab of `path` again in workspace `workspace` of `window`; its tab's id, or none when the file is
/// gone.
pub(crate) fn reopen(window: &Rc<Window>, workspace: u64, path: &Path) -> Option<u64> {
    if !path.is_file() {
        return None;
    }
    let host = NSView::new(app().mtm);
    let key = term_ui_editor::tab_key(path);
    let title = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let id = window.add_hosted(workspace, TabKind::File, Some(key), &title, host.clone())?;
    let handle = Handle::new(WorkspaceId::new(workspace));
    if term_ui_editor::attach_restored(&handle, TabId::new(id), &host, path).is_none() {
        window.close_hosted(workspace, id);
        return None;
    }
    Some(id)
}

/// Opens in editor tabs the copies of unsaved text a crash left behind, each in the workspace that holds its file,
/// else in the one in front, marked unsaved so that the user saves or discards them.
pub(crate) fn offer_recovered() {
    let pending = term_ui_editor::pending_copies();
    if pending.is_empty() {
        return;
    }
    let Some(app) = try_app() else { return };
    let front = app.front().and_then(|window| selected(&window));
    let windows = app.windows.borrow().clone();
    for copy in pending {
        let holder = copy.path.as_ref().or(copy.project.as_ref()).and_then(|path| {
            windows.iter().find_map(|window| {
                let workspace = window
                    .workspaces()
                    .into_iter()
                    .find(|workspace| path.starts_with(&workspace.base))?;
                Some(Handle::new(WorkspaceId::new(workspace.id)))
            })
        });
        if let Some(workspace) = holder.or(front) {
            term_ui_editor::open_recovered(&workspace, &copy.id);
        }
    }
}
