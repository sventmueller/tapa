//! The shape rules of the sources: no Rust file over 1000 lines, and no name left over from Dango's copy (E2).

use std::path::{Path, PathBuf};

use crate::root;

/// The most lines a Rust file may have; a longer one is two files.
const MOST_LINES: usize = 1000;

fn rust_files(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name == "vendor" || name == "fuzz" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            rust_files(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// Fails with every file that breaks a rule.
pub(crate) fn check() -> Result<(), String> {
    let mut files = Vec::new();
    for dir in ["src", "crates", "xtask"] {
        rust_files(&root().join(dir), &mut files);
    }
    let mut broken = Vec::new();
    for file in &files {
        let checks_names = !file.ends_with("xtask/src/structure.rs");
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let lines = text.lines().count();
        if lines > MOST_LINES {
            broken.push(format!("{}: {lines} lines, at most {MOST_LINES}", file.display()));
        }
        for (number, line) in text.lines().enumerate() {
            if checks_names && leftover(line) {
                broken.push(format!(
                    "{}:{}: a name from Dango: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    if broken.is_empty() {
        println!("structure: {} files fine", files.len());
        Ok(())
    } else {
        Err(broken.join("\n"))
    }
}

/// Whether `line` names one of Dango's crates, which the copy renamed.
fn leftover(line: &str) -> bool {
    [
        "ide_acp",
        "ide_claude",
        "ide_codex",
        "ide_core",
        "ide_document",
        "ide_git",
        "ide_lsp",
        "ide_opencode",
        "ide_process",
        "ide_product",
        "ide_project",
        "ide_session",
        "ide_terminal",
        "ide_thread",
        "ide_ui_",
        "ide_ui_browser",
        "ide_ui_dictation",
        "ide_vim",
        "ide-acp",
        "ide-claude",
        "ide-codex",
        "ide-core",
        "ide-document",
        "ide-git",
        "ide-lsp",
        "ide-opencode",
        "ide-process",
        "ide-project",
        "ide-session",
        "ide-thread",
        "ide-ui-",
        "ide-ui-browser",
        "ide-ui-dictation",
        "ide-decoder",
        "ide-workspace-hack",
        "ideBrowser",
        "ideDictat",
        "IDEDictation",
        "IDEBrowser",
        "IDEEditor",
        "IDEFileTree",
        "ideLoadWebPictures",
        "ideMarkdown",
        "IDEPreviewPage",
        "ideShowPreview",
        "ide_decoder",
        "ide_workspace_hack",
    ]
    .iter()
    .any(|name| line.contains(name))
}

#[cfg(test)]
mod tests {
    #[test]
    fn dangos_crate_names_are_found_and_shell_variables_are_not() {
        assert!(super::leftover("use ide_core::paths;"));
        assert!(super::leftover("#[unsafe(method(ideBrowserReload:))]"));
        assert!(!super::leftover("IDE_ZSH_ZDOTDIR"));
    }
}
