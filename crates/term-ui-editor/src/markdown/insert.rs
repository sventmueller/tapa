//! What the format bar inserts after asking: a table of a chosen size, a picture from a file, and splat's starter
//! diagrams.

use std::path::Path;

use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSModalResponseOK, NSOpenPanel, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use splat_markdown::commands;
use term_ui::kit::alert::{Alert, Role, first};

use super::{apply, doc_dir, text_and_selection};
use crate::EditorTextView;
use crate::util::ns;

/// A starter diagram, as splat offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Diagram {
    Flowchart,
    Sequence,
    Gantt,
}

impl Diagram {
    pub(crate) const ALL: [Diagram; 3] = [Diagram::Flowchart, Diagram::Sequence, Diagram::Gantt];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Diagram::Flowchart => t("Flussdiagramm"),
            Diagram::Sequence => t("Sequenzdiagramm"),
            Diagram::Gantt => t("Gantt-Diagramm"),
        }
    }

    /// The Mermaid code it starts with, the words in the interface's language.
    pub(crate) fn template(self) -> String {
        match self {
            Diagram::Flowchart => format!(
                "flowchart TD\n    A[{}] --> B{{{}}}\n    B -->|{}| C[{}]\n    B -->|{}| D[{}]",
                t("Start"),
                t("Entscheidung"),
                t("Ja"),
                t("Weiter"),
                t("Nein"),
                t("Ende")
            ),
            Diagram::Sequence => format!(
                "sequenceDiagram\n    Alice->>Bob: {}\n    Bob-->>Alice: {}",
                t("Anfrage"),
                t("Antwort")
            ),
            Diagram::Gantt => format!(
                "gantt\n    title {}\n    dateFormat YYYY-MM-DD\n    section {}\n    {} A :a1, 2026-01-05, 10d\n    {} B :after a1, 5d",
                t("Projektplan"),
                tf("Phase {}", &["1"]),
                t("Aufgabe"),
                t("Aufgabe")
            ),
        }
    }
}

/// Asks for the size of a table and inserts it at the selection of `view`.
pub(super) fn table(view: &EditorTextView) {
    let mtm = view.mtm();
    let rows = number_field(view, "3");
    let columns = number_field(view, "3");
    let form = NSStackView::new(mtm);
    form.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    form.setSpacing(term_ui::kit::style::space::S);
    for (title, field) in [(t("Zeilen (inklusive Kopfzeile):"), &rows), (t("Spalten:"), &columns)] {
        let label = NSTextField::labelWithString(&ns(title), mtm);
        let row = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*label as &NSView, field]), mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        form.addArrangedSubview(&row);
    }
    form.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 54.0)));
    let answer = Alert::new(mtm, t("Tabelle einfügen"))
        .informative(t("Größe der neuen Tabelle festlegen."))
        .button(t("Einfügen"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&form)
        .focus(&rows)
        .run_modal();
    if !first(answer) {
        return;
    }
    let (rows, columns) = table_size(rows.integerValue(), columns.integerValue());
    let (text, sel) = text_and_selection(view);
    apply(
        view,
        &commands::insert_table(&text, sel, rows, columns),
        t("Tabelle einfügen"),
    );
}

fn number_field(view: &EditorTextView, value: &str) -> objc2::rc::Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&ns(value), view.mtm());
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(60.0, 24.0)));
    field
}

/// The size of a new table from what was typed: it needs a header and a row, and a field that holds no number counts
/// as 0.
fn table_size(rows: isize, columns: isize) -> (usize, usize) {
    let clamp = |n: isize, low: isize, high: isize| n.clamp(low, high).unsigned_abs();
    (clamp(rows, 2, 100), clamp(columns, 1, 30))
}

/// Asks for a picture file and links it at the selection of `view`, by a path from the document's folder.
pub(super) fn image(view: &EditorTextView) {
    let mtm = view.mtm();
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setPrompt(Some(&ns(t("Einfügen"))));
    panel.setMessage(Some(&ns(t("Bild auswählen, das in das Dokument eingefügt wird."))));
    if let Some(dir) = doc_dir(view) {
        panel.setDirectoryURL(Some(&objc2_foundation::NSURL::fileURLWithPath(&ns(
            &dir.to_string_lossy()
        ))));
    }
    if panel.runModal() != NSModalResponseOK {
        return;
    }
    let Some(path) = panel.URL().and_then(|url| url.path()) else {
        return;
    };
    let path = std::path::PathBuf::from(path.to_string());
    let Some((alt, relative)) = picture_link(doc_dir(view).as_deref(), &path) else {
        objc2_app_kit::NSBeep();
        return;
    };
    let (text, sel) = text_and_selection(view);
    apply(
        view,
        &commands::insert_image(&text, sel, &alt, &relative),
        t("Bild einfügen"),
    );
}

/// The alternative text and the link of the picture `path` in a document in `dir`; `None` for a file that is no picture.
fn picture_link(dir: Option<&Path>, path: &Path) -> Option<(String, String)> {
    if !is_picture(path) {
        return None;
    }
    let alt = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some((alt, super::files::relative_path(dir, path)))
}

/// Whether `path` names a picture a preview shows, by its extension.
fn is_picture(path: &Path) -> bool {
    const PICTURES: [&str; 11] = [
        "png", "jpg", "jpeg", "gif", "webp", "svg", "heic", "tif", "tiff", "bmp", "avif",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PICTURES.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_known_by_extension_in_any_case() {
        assert!(is_picture(Path::new("a/Bild.PNG")));
        assert!(is_picture(Path::new("x.svg")));
        assert!(!is_picture(Path::new("notes.md")));
        assert!(!is_picture(Path::new("png")));
    }

    #[test]
    fn a_picture_links_by_its_name_from_the_documents_folder_and_another_file_does_not() {
        let (alt, link) = picture_link(Some(Path::new("/doc")), Path::new("/doc/img/Logo.png")).expect("a picture");
        assert_eq!(alt, "Logo");
        assert!(link.ends_with("img/Logo.png") && !link.starts_with("/doc"), "{link}");
        assert!(picture_link(Some(Path::new("/doc")), Path::new("/doc/notes.md")).is_none());
    }

    #[test]
    fn every_starter_diagram_has_a_title_and_mermaid_code_of_its_kind() {
        let starts = ["flowchart TD", "sequenceDiagram", "gantt"];
        for (diagram, start) in Diagram::ALL.into_iter().zip(starts) {
            assert!(!diagram.title().is_empty());
            let code = diagram.template();
            assert!(code.starts_with(start), "{code}");
            assert!(code.lines().count() >= 3, "{code}");
        }
    }

    #[test]
    fn a_table_has_a_header_and_a_row_and_stays_within_bounds() {
        assert_eq!(table_size(0, 0), (2, 1));
        assert_eq!(table_size(-4, 3), (2, 3));
        assert_eq!(table_size(5, 4), (5, 4));
        assert_eq!(table_size(500, 99), (100, 30));
    }
}
