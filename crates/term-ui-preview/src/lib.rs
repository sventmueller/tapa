//! Documents as they read (E22): a Markdown document's preview beside its editor, as native text with the formulas,
//! diagrams and pictures of splat's render path, which scrolls with the editor; pictures decoded by splat's sandboxed
//! service, PDF files through PDFKit, and sound, films, fonts and office documents through Quick Look, each in a tab
//! of its own.

mod image;
mod links;
mod markdown;
mod page;
mod pdf;
mod quick_look;
mod sync;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_ui::workspace_host::{TabKind, Tabs, Workspace, WorkspaceId};
use term_ui_editor::EditorSerial;

use markdown::Preview;

/// The open Markdown previews of all workspaces.
struct Previews {
    open: RefCell<Vec<Rc<Preview>>>,
    next_key: Cell<u64>,
    looking: Cell<bool>,
    /// The size of the editors' text, which the previews show theirs in proportion to.
    font_size: Cell<f64>,
}

thread_local! {
    static PREVIEWS: Previews = const {
        Previews {
            open: RefCell::new(Vec::new()),
            next_key: Cell::new(1),
            looking: Cell::new(false),
            font_size: Cell::new(DEFAULT_FONT_SIZE),
        }
    };
}

/// The editors' default text size, at which a preview shows splat's own.
const DEFAULT_FONT_SIZE: f64 = 13.0;

/// How often the picture files of the open previews are compared with what they show, as in splat.
const LOOK_AT_IMAGES: Duration = Duration::from_secs(2);

/// Tells the render path where its page lies and where failures go, before any window opens.
pub fn start(mtm: MainThreadMarker) {
    splat_render::start(splat_render::Host::of_main_bundle(|what, detail| {
        term_base::diag::report(what, detail);
    }));
    splat_render::images::watch_memory(mtm);
}

/// Whether `path` is a Markdown document by its extension.
pub fn is_markdown(path: &Path) -> bool {
    has_extension(path, &["md", "markdown", "mdown", "mkd", "mkdn"])
}

fn has_extension(path: &Path, known: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| known.contains(&e.to_ascii_lowercase().as_str()))
}

/// Whether `path` opens in a tab that shows it rather than in an editor.
pub fn is_document(path: &Path) -> bool {
    image::is_image(path) || pdf::is_pdf(path) || quick_look::is_quick_look(path)
}

/// Opens `path` in a tab of `workspace` that shows it, or selects the tab that does; `false` for a file an editor
/// should take.
pub fn open_document(workspace: WorkspaceId, path: &Path) -> bool {
    if !is_document(path) {
        return false;
    }
    let workspace = Workspace::new(workspace);
    let key = format!("view:{}", term_ui_editor::tab_key(path));
    if workspace.select_tab(&key) {
        return true;
    }
    let mtm = workspace.mtm();
    let name = display_name(path);
    let view = if image::is_image(path) {
        Some(image::view(mtm, path, &name))
    } else if pdf::is_pdf(path) {
        pdf::view(mtm, path, &name).or_else(|| {
            cannot_open(mtm, &name, splat_i18n::t("Die Datei ist kein lesbares PDF."));
            None
        })
    } else {
        quick_look::view(mtm, path, &name)
    };
    let Some(view) = view else {
        return pdf::is_pdf(path);
    };
    let _ = workspace.add_tab(TabKind::File, Some(key), &name, view);
    true
}

/// Opens the drawing of the SVG file `path`, decoded by the sandboxed service, in a tab of its own apart from an
/// editor of its source, or selects it.
pub fn open_drawing(workspace: WorkspaceId, path: &Path) {
    let workspace = Workspace::new(workspace);
    let key = format!("drawing:{}", term_ui_editor::tab_key(path));
    if workspace.select_tab(&key) {
        return;
    }
    let name = display_name(path);
    let view = image::view(workspace.mtm(), path, &name);
    let title = splat_i18n::tf("Vorschau: {}", &[&name]);
    let _ = workspace.add_tab(TabKind::File, Some(key), &title, view);
}

fn cannot_open(mtm: MainThreadMarker, name: &str, why: &str) {
    term_ui::kit::alert::warn(mtm, &splat_i18n::tf("„{}“ lässt sich nicht öffnen.", &[name]), why);
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Shows the preview of the Markdown document of `editor` beside it, or takes away the one it has; whether one shows
/// now.
pub fn toggle_beside(workspace: WorkspaceId, editor: EditorSerial) -> bool {
    if close_beside(editor) {
        return false;
    }
    open_beside(workspace, editor);
    let shown = is_beside(editor);
    if let Some(view) = term_ui_editor::text_view(editor)
        && let Some(window) = view.window()
    {
        window.makeFirstResponder(Some(&view));
    }
    shown
}

/// Shows the preview of the Markdown document of `editor` beside it unless one shows.
pub fn open_beside(workspace: WorkspaceId, editor: EditorSerial) {
    if is_beside(editor) {
        return;
    }
    let Some((Some(path), _)) = term_ui_editor::text_of(editor) else {
        return;
    };
    if !is_markdown(&path) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let key = PREVIEWS.with(|p| p.next_key.replace(p.next_key.get() + 1));
    let preview = Preview::new(mtm, workspace, editor, &path, key, zoom());
    let page: Retained<NSView> = Retained::into_super(preview.views.page.clone());
    PREVIEWS.with(|p| p.open.borrow_mut().push(preview.clone()));
    term_ui_editor::set_beside(editor, Some(&page));
    sync::attach(&preview);
    preview.lay_out();
    look_at_images_later();
}

/// Takes away the preview beside `editor`; whether there was one.
pub fn close_beside(editor: EditorSerial) -> bool {
    let closed: Vec<Rc<Preview>> = PREVIEWS.with(|p| {
        let mut open = p.open.borrow_mut();
        let (closed, kept) = open.drain(..).partition(|preview| preview.editor() == editor);
        *open = kept;
        closed
    });
    for preview in &closed {
        preview.close();
    }
    if !closed.is_empty() {
        term_ui_editor::set_beside(editor, None);
    }
    !closed.is_empty()
}

/// Whether a preview shows beside `editor`.
pub fn is_beside(editor: EditorSerial) -> bool {
    find(|p| p.editor() == editor).is_some()
}

/// The text of `editor` changed: the preview beside it lays it out again.
pub fn edited(editor: EditorSerial) {
    if let Some(preview) = find(|p| p.editor() == editor) {
        preview.text_changed();
    }
}

/// The editor `editor` is about to close, and its preview with it.
pub fn closing(editor: EditorSerial) {
    close_beside(editor);
}

/// Shows the open previews as large as editors at `font_size` points.
pub fn font_size_changed(font_size: f64) {
    PREVIEWS.with(|p| p.font_size.set(font_size));
    for preview in all() {
        preview.set_zoom(zoom());
    }
}

/// How much larger than splat's own size a preview shows its text.
fn zoom() -> f64 {
    PREVIEWS.with(|p| p.font_size.get()) / DEFAULT_FONT_SIZE
}

/// The user asked for the pictures from the web of the preview `key`.
fn allow_web_pictures(key: u64) {
    with(key, |preview| preview.allow_web_pictures());
}

fn appearance_changed(key: u64) {
    with(key, |preview| preview.lay_out());
}

fn width_changed(key: u64) {
    with(key, |preview| preview.render_soon());
}

fn drag_ended(key: u64) {
    with(key, |preview| preview.views.page.drag_ended());
}

/// Runs `f` with the open preview `key`, if there is one.
fn with(key: u64, f: impl FnOnce(&Rc<Preview>)) {
    if let Some(preview) = find(|p| p.key == key) {
        f(&preview);
    }
}

fn find(matches: impl Fn(&Preview) -> bool) -> Option<Rc<Preview>> {
    PREVIEWS.with(|p| p.open.borrow().iter().find(|preview| matches(preview)).cloned())
}

/// The open previews; those whose workspace or editor went leave on the way.
fn all() -> Vec<Rc<Preview>> {
    let gone: Vec<Rc<Preview>> = PREVIEWS.with(|p| {
        let mut open = p.open.borrow_mut();
        let (gone, kept) = open.drain(..).partition(|preview| preview.is_closed());
        *open = kept;
        gone
    });
    for preview in gone {
        preview.close();
    }
    PREVIEWS.with(|p| p.open.borrow().clone())
}

/// Compares the picture files of the open previews with what they show, and looks again where their links lead, every
/// two seconds while any is open.
fn look_at_images_later() {
    if PREVIEWS.with(|p| p.looking.replace(true)) {
        return;
    }
    term_base::main_after(LOOK_AT_IMAGES, || {
        PREVIEWS.with(|p| p.looking.set(false));
        let open = all();
        if open.is_empty() {
            return;
        }
        splat_render::images::look_at_images(open.iter().map(|p| (p.viewer(), p.images())));
        for preview in &open {
            preview.look_at_links_again();
        }
        look_at_images_later();
    });
}

/// What the self-test reads of the previews and the decoding service.
#[cfg(feature = "selftest")]
pub mod selftest;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_known_by_extension_in_any_case() {
        assert!(is_markdown(Path::new("README.MD")));
        assert!(is_markdown(Path::new("notes.mkdn")));
        assert!(!is_markdown(Path::new("md")));
    }

    #[test]
    fn documents_open_in_a_viewer_but_source_files_in_an_editor() {
        for shown in ["a.pdf", "b.PNG", "c.docx", "d.mp4", "e.webp"] {
            assert!(is_document(Path::new(shown)), "{shown}");
        }
        // Quick Look would take `.ts` as video and `.key` as Keynote; an SVG is source first.
        for edited in ["main.ts", "server.key", "a.md", "Makefile", "logo.svg"] {
            assert!(!is_document(Path::new(edited)), "{edited}");
        }
    }
}
