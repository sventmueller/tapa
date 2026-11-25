//! What the self-test reads from and does with a workspace's file list, through the code its rows and menu run, and
//! Dango's `file-icons` scenario: a folder with files of many kinds, listed with their logos and symbols.

use std::path::Path;

use objc2::DefinedClass;
use term_ui::workspace_host::AsWorkspace;

/// A file of each kind, as the list sorts them after the one folder.
pub const ICON_FILES: &[&str] = &[
    ".editorconfig",
    ".env",
    ".gitignore",
    "api.proto",
    "app.rb",
    "app.ts",
    "app.tsx",
    "App.vue",
    "build.sh",
    "Cargo.lock",
    "Cargo.toml",
    "CHANGELOG.md",
    "chime.wav",
    "data.csv",
    "Dockerfile",
    "fix.patch",
    "index.html",
    "index.js",
    "index.php",
    "Info.plist",
    "Inter.ttf",
    "intro.mov",
    "lexer.c",
    "lexer.h",
    "LICENSE-MIT",
    "logo.svg",
    "main.dart",
    "main.go",
    "Main.hs",
    "Main.java",
    "Main.kt",
    "main.rs",
    "Makefile",
    "manual.pdf",
    "notes.md",
    "notes.txt",
    "package.json",
    "parser_test.go",
    "photo.png",
    "Program.cs",
    "README.md",
    "release.zip",
    "rustfmt.toml",
    "schema.prisma",
    "schema.sql",
    "site.css",
    "TODO.md",
    "tool.py",
    "View.swift",
    "workflow.yml",
];

/// How many rows the file list of `workspace` shows, `None` while it is not built.
pub fn row_count(workspace: &(impl AsWorkspace + ?Sized)) -> Option<isize> {
    let panel = crate::panel::of(workspace.as_workspace())?;
    Some(panel.outline()?.numberOfRows())
}

/// The names the rows of the file list of `workspace` show, top to bottom.
pub fn row_names(workspace: &(impl AsWorkspace + ?Sized)) -> Vec<String> {
    let Some(view) = crate::panel::of(workspace.as_workspace()).and_then(|panel| panel.outline()) else {
        return Vec::new();
    };
    (0..view.numberOfRows())
        .filter_map(|row| view.itemAtRow(row))
        .filter_map(|item| crate::view::path_of(&item))
        .filter_map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .collect()
}

/// Clicks the row of `path` in the file list of `workspace`, twice for a double click, as the mouse does; whether
/// the list shows a row for it.
pub fn click(workspace: &(impl AsWorkspace + ?Sized), path: &Path, double: bool) -> bool {
    let Some(panel) = crate::panel::of(workspace.as_workspace()) else {
        return false;
    };
    let Some(view) = panel.outline() else { return false };
    let Some(item) = panel.item_of(path) else { return false };
    // SAFETY: The item is one the data source handed to this view, kept alive in its items.
    let row = unsafe { view.rowForItem(Some(&item)) };
    if row < 0 {
        return false;
    }
    panel.row_clicked(&view, row);
    if double {
        panel.row_double_clicked(&view, row);
    }
    true
}

/// The titles of the context menu of the row of `path` in the file list of `workspace`.
pub fn menu_titles(workspace: &(impl AsWorkspace + ?Sized), path: &Path) -> Vec<String> {
    let Some(panel) = crate::panel::of(workspace.as_workspace()) else {
        return Vec::new();
    };
    let kind = if path.is_dir() {
        term_project::files_panel::Kind::Folder
    } else {
        term_project::files_panel::Kind::File
    };
    let entries = panel.ivars().model.borrow_mut().open_menu(Some(path), Some(kind), &[]);
    entries
        .into_iter()
        .filter_map(|entry| match entry {
            term_project::files_panel::MenuEntry::Item { title, .. } => Some(title.to_string()),
            term_project::files_panel::MenuEntry::Separator => None,
        })
        .collect()
}

/// Whether the watcher of the file list of `workspace` runs, which it does only while the list is built.
pub fn watching(workspace: &(impl AsWorkspace + ?Sized)) -> bool {
    crate::panel::of(workspace.as_workspace()).is_some_and(|panel| panel.ivars().project.is_watching())
}

/// The checks of the `file-icons` scenario over [`ICON_FILES`]: many different icons, each with an image of a symbol
/// macOS knows. What went wrong, else how many icons.
pub fn icon_check() -> Result<String, String> {
    let kinds: std::collections::HashSet<_> = ICON_FILES.iter().map(|file| term_ui_icons::icon_of(file)).collect();
    if kinds.len() * 2 <= ICON_FILES.len() {
        return Err(format!("{} icons for {} files", kinds.len(), ICON_FILES.len()));
    }
    let missing: Vec<_> = kinds
        .iter()
        .filter(|icon| term_ui_icons::template(icon).is_none())
        .map(|icon| icon.id)
        .collect();
    if !missing.is_empty() {
        return Err(format!("no image for {}", missing.join(", ")));
    }
    Ok(format!("{} icons for {} files", kinds.len(), ICON_FILES.len()))
}
