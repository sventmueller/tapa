//! The self-test's steps for files, the sidebar's panels and git (E22), after Dango's `git` scenario and its flows
//! without commit messages by an agent and secrets: `gitrepo:<path>` makes a repository there with a committed
//! `README.md` and an untracked `a.txt`; `open:<path>` opens a file of the workspace in front, relative to its base
//! path; `editortext:<name>` prints the text of the editor in front and whether it is unsaved, `editortype:<text>`
//! types into it, `editorscroll:<end|top|name>` scrolls it to the end or the top of its text, or not, and prints where
//! its first line then sits, `save` saves it and prints the file; `panel:<files|git>` shows that panel of the selected
//! workspace in place of the sidebar's list and `panel:back` the list again; `files:<name>` prints the
//! file list's rows and whether its watcher runs, `filesclick:<path>` clicks a row, twice with a trailing `!`;
//! `gitpanel:<name>` prints the Git panel's sections, `gitstage:<file>` stages a file, `gitcommit:<message>` commits;
//! `branchmenu:<name>` prints the toolbar's branch pull-down; `worktree:<branch>` chooses "Neuer Worktree …" there and
//! answers its sheet with `<branch>`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSTextField, NSView, NSWindow};
use term_ui::workspace_host::{TabId, TabKind};
use term_ui_sidebar::Panel;

use crate::state::app;
use crate::window::Window;
use crate::workspace::Workspace;

/// Runs `step` if it is one of these; whether it was.
pub(super) fn run(step: &str) -> bool {
    if step == "save" || step == "save:" {
        save();
        return true;
    }
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    let Some(window) = app().front() else {
        return false;
    };
    match name {
        "gitrepo" => print_result("gitrepo", make_repository(Path::new(argument))),
        "open" => open(&window, argument),
        "editortext" => editor_text(&window, argument),
        "editortype" => match window
            .selected_workspace()
            .and_then(|workspace| editor_in_front(&workspace))
        {
            Some(editor) => term_ui_editor::selftest::type_text(editor, argument),
            None => println!("editortype: no editor in front"),
        },
        "editorscroll" => editor_scroll(&window, argument),
        "panel" => match argument {
            "files" => show_panel(&window, Panel::Files),
            "git" => show_panel(&window, Panel::Git),
            "back" => {
                window.panel_back();
                println!("panel back: sidebar {}", window.sidebar_state());
            }
            _ => eprintln!("selftest: no panel {argument}"),
        },
        "files" => files(&window, argument),
        "filesclick" => files_click(&window, argument),
        "gitpanel" => git_panel(&window, argument),
        "gitstage" => on_workspace(&window, |workspace| {
            term_ui_git::selftest::stage(&workspace.handle(), Path::new(argument));
        }),
        "gitcommit" => on_workspace(&window, |workspace| {
            term_ui_git::selftest::commit(&workspace.handle(), argument);
        }),
        "branchmenu" => on_workspace(&window, |workspace| {
            let seen = term_ui_git::toolbar_seen(&workspace.handle());
            println!("branchmenu {argument}: {seen:?}");
        }),
        "worktree" => worktree(&window, argument),
        _ => return false,
    }
    true
}

/// How long the steps after `worktree:` wait for git to add it and its workspace to open.
pub(super) const WORKTREE_TIME: Duration = Duration::from_millis(2500);

fn print_result(name: &str, result: Result<String, String>) {
    match result {
        Ok(done) => println!("{name}: {done}"),
        Err(error) => println!("{name}: failed: {error}"),
    }
}

fn on_workspace(window: &Window, act: impl FnOnce(&Workspace)) {
    match window.selected_workspace() {
        Some(workspace) => act(&workspace),
        None => eprintln!("selftest: no workspace"),
    }
}

/// git for the test repository, without the user's or the system's configuration.
fn git(folder: &Path, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new("git")
        .current_dir(folder)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// A repository at `folder`, made anew: `README.md` committed on `main`, `a.txt` untracked.
fn make_repository(folder: &Path) -> Result<String, String> {
    if folder.exists() {
        let _ = std::fs::remove_dir_all(folder);
    }
    std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    git(folder, &["init", "--quiet", "--initial-branch=main"])?;
    git(folder, &["config", "user.name", "Selftest"])?;
    git(folder, &["config", "user.email", "selftest@example.invalid"])?;
    git(folder, &["config", "commit.gpgsign", "false"])?;
    std::fs::write(
        folder.join("README.md"),
        "# Selftest\n\nA repository of the self-test.\n",
    )
    .map_err(|error| error.to_string())?;
    git(folder, &["add", "README.md"])?;
    git(folder, &["commit", "--quiet", "-m", "First commit"])?;
    std::fs::write(folder.join("a.txt"), "a new file\n").map_err(|error| error.to_string())?;
    Ok(folder.display().to_string())
}

/// `argument` as a path: absolute as it is, else below the selected workspace's base path.
fn path_in(window: &Window, argument: &str) -> PathBuf {
    let path = PathBuf::from(argument);
    if path.is_absolute() {
        return path;
    }
    window
        .selected_workspace()
        .map_or(path.clone(), |workspace| workspace.base.join(&path))
}

fn open(window: &Window, argument: &str) {
    let path = path_in(window, argument);
    crate::editor_tab::open(window, &path);
    println!("open {}", path.display());
}

fn editor_text(window: &Window, name: &str) {
    let Some(workspace) = window.selected_workspace() else {
        return println!("editortext {name}: no workspace");
    };
    let Some(editor) = editor_in_front(&workspace) else {
        return println!("editortext {name}: no editor in front");
    };
    let (shown, held) = term_ui_editor::selftest::texts(editor).unwrap_or_default();
    println!(
        "editortext {name}: {shown:?} (document {}, unsaved {})",
        if shown == held { "the same" } else { "differs" },
        term_ui_editor::selftest::is_dirty(editor)
    );
}

/// Scrolls the editor in front to the end of its text with `end`, to the beginning with `top`, as the End and Home
/// keys do, or not at all with any other word, and prints where the top of its first line then is, in points below
/// the top of the scroll view.
fn editor_scroll(window: &Window, to: &str) {
    let editor = window
        .selected_workspace()
        .and_then(|workspace| editor_in_front(&workspace));
    let Some((editor, view)) = editor.and_then(|editor| Some((editor, term_ui_editor::text_view(editor)?))) else {
        return println!("editorscroll {to}: no editor in front");
    };
    use objc2_app_kit::NSStandardKeyBindingResponding;
    let Some(scroll) = view.enclosingScrollView() else {
        return println!("editorscroll {to}: no scroll view");
    };
    // SAFETY: The text view's own scroll actions on the main thread; they take no sender.
    unsafe {
        match to {
            "top" => view.scrollToBeginningOfDocument(None),
            "end" => view.scrollToEndOfDocument(None),
            _ => {}
        }
    }
    view.displayIfNeeded();
    let first = term_ui_editor::byte_y(editor, 0, true).unwrap_or_default();
    let at = view.convertPoint_toView(objc2_foundation::NSPoint::new(0.0, first), Some(&scroll));
    let below = if scroll.isFlipped() {
        at.y
    } else {
        scroll.bounds().size.height - at.y
    };
    println!(
        "editorscroll {to}: first line {below:.1} below the top, content inset {:.1}, clip at {:.1}",
        scroll.contentInsets().top,
        scroll.contentView().bounds().origin.y
    );
}

/// The scroll view of the editor in the front window's tab in front.
pub(super) fn front_editor_scroll() -> Option<Retained<objc2_app_kit::NSScrollView>> {
    let workspace = app().front()?.selected_workspace()?;
    term_ui_editor::text_view(editor_in_front(&workspace)?)?.enclosingScrollView()
}

/// The editor of `workspace`'s tab in front.
fn editor_in_front(workspace: &Workspace) -> Option<term_ui_editor::EditorSerial> {
    let tab = workspace.selected_tab()?;
    tab.as_hosted().filter(|hosted| hosted.kind() == TabKind::File)?;
    let (_, editor) = term_ui_editor::tab_document(&workspace.handle(), TabId::new(tab.id))?;
    Some(editor)
}

fn save() {
    let Some(workspace) = app().front().and_then(|window| window.selected_workspace()) else {
        return println!("save: no workspace");
    };
    let Some(editor) = editor_in_front(&workspace) else {
        return println!("save: no editor in front");
    };
    let written = term_ui_editor::selftest::save(editor, false);
    let file = term_ui_editor::text_of(editor)
        .and_then(|(path, _)| path)
        .and_then(|path| std::fs::read_to_string(path).ok());
    println!(
        "save: written {written}, unsaved {}, file {file:?}",
        term_ui_editor::selftest::is_dirty(editor)
    );
}

fn show_panel(window: &std::rc::Rc<Window>, panel: Panel) {
    on_workspace(window, |workspace| {
        window.set_panel_open(workspace.id, panel, true);
        println!("panel {panel:?}: sidebar {}", window.sidebar_state());
    });
}

fn files(window: &Window, name: &str) {
    on_workspace(window, |workspace| {
        let handle = workspace.handle();
        println!(
            "files {name}: watching {}, rows {:?}",
            term_ui_files::selftest::watching(&handle),
            term_ui_files::selftest::row_names(&handle)
        );
    });
}

fn files_click(window: &Window, argument: &str) {
    let (argument, double) = argument
        .strip_suffix('!')
        .map_or((argument, false), |argument| (argument, true));
    let path = path_in(window, argument);
    on_workspace(window, |workspace| {
        let clicked = term_ui_files::selftest::click(&workspace.handle(), &path, double);
        println!("filesclick {argument}: row {clicked}, double {double}");
    });
}

fn git_panel(window: &Window, name: &str) {
    on_workspace(window, |workspace| {
        let Some(seen) = term_ui_git::selftest::seen(&workspace.handle()) else {
            return println!("gitpanel {name}: no panel");
        };
        println!(
            "gitpanel {name}: {} on {:?}, staged {:?}, changes {:?}, commits {}, top {:?}, problem {:?}, busy {}, \
             untrusted note {}",
            seen.phase,
            seen.branch,
            seen.staged,
            seen.changes,
            seen.commits,
            seen.top,
            seen.problem,
            seen.busy,
            term_ui_git::selftest::untrusted_note(&workspace.handle())
        );
    });
}

/// Chooses "Neuer Worktree …" in the toolbar's branch pull-down and gives its sheet `branch`.
fn worktree(window: &Window, branch: &str) {
    let Some(workspace) = window.selected_workspace() else {
        return println!("worktree: no workspace");
    };
    let chosen = term_ui_git::toolbar_choose(&workspace.handle(), splat_i18n::t("Neuer Worktree …"));
    println!("worktree {branch}: offered {chosen}");
    let (branch, window_id) = (branch.to_string(), window.id());
    term_base::main_after(Duration::from_millis(300), move || {
        let Some(window) = crate::state::try_app().and_then(|app| app.window(window_id)) else {
            return;
        };
        let Some(sheet) = window.front_ns_window().and_then(|ns| ns.attachedSheet()) else {
            return println!("worktree {branch}: no sheet");
        };
        println!("worktree {branch}: answered {}", answer(&sheet, &branch));
    });
}

/// Types `text` into the sheet's text field and presses its first button; whether both were there.
fn answer(sheet: &NSWindow, text: &str) -> bool {
    let Some(content) = sheet.contentView() else {
        return false;
    };
    let mut fields = Vec::new();
    let mut buttons = Vec::new();
    collect(&content, &mut fields, &mut buttons);
    let Some(field) = fields.into_iter().find(|field| field.isEditable()) else {
        return false;
    };
    field.setStringValue(&term_ui::kit::ns(text));
    let Some(button) = buttons
        .into_iter()
        .find(|button| button.title().to_string() == splat_i18n::t("Anlegen"))
    else {
        return false;
    };
    // SAFETY: A click on the sheet's button, whose target is the alert, as a person's click would be.
    unsafe { button.performClick(None) };
    true
}

fn collect(view: &NSView, fields: &mut Vec<Retained<NSTextField>>, buttons: &mut Vec<Retained<NSButton>>) {
    for child in view.subviews().iter() {
        if let Ok(field) = child.clone().downcast::<NSTextField>() {
            fields.push(field);
        } else if let Ok(button) = child.clone().downcast::<NSButton>() {
            buttons.push(button);
        }
        collect(&child, fields, buttons);
    }
}
