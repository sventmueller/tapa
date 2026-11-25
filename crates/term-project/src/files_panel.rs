//! What the app's files panel decides without AppKit: what a click does, what the context menu offers and what a new name
//! for a file means; the controller carries out the effects.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use splat_i18n::{t, tf};
use term_base::explain::Explain;

use crate::tree::Entry;

/// What the tree knows of a row besides its path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Row {
    /// Whether it is a folder that can open.
    pub expandable: bool,
    /// Ignored by git or hidden, drawn dimmer.
    pub ignored: bool,
}

/// What a row stands for on disk, as far as the model needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A folder.
    Folder,
    /// Anything else.
    File,
}

/// A command of the context menu and the panel's title row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open the file in a new pane right of the document in front.
    OpenBeside,
    /// Open the file in a new pane below the document in front.
    OpenBelow,
    /// Open an SVG file's drawing.
    OpenAsPreview,
    /// Open an HTML page in a browser tab, served by the app (E131).
    OpenInBrowserTab,
    /// Make a new file.
    NewFile,
    /// Make a new folder.
    NewFolder,
    /// Show the row in the Finder.
    Reveal,
    /// Copy the full path.
    CopyPath,
    /// Copy the path below the folder.
    CopyRelativePath,
    /// Give the row a new name.
    Rename,
    /// Move the row to the Trash.
    Trash,
}

/// A line of the context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    /// A dividing line.
    Separator,
    /// An entry that runs a command.
    Item {
        /// What the entry says.
        title: &'static str,
        /// What it runs.
        command: Command,
        /// Whether it can be chosen.
        enabled: bool,
    },
}

/// What the user wants a name for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    /// A new file or folder in `parent`.
    Create {
        /// The folder it goes into.
        parent: PathBuf,
        /// A file or a folder.
        kind: Kind,
    },
    /// A new name for `path`.
    Rename {
        /// What is renamed.
        path: PathBuf,
    },
}

/// Something the controller does for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open the folder of the clicked row, or close it.
    Toggle,
    /// Open the file in the preview tab.
    Preview(PathBuf),
    /// Open the file in a tab of its own.
    Open(PathBuf),
    /// Open the file in a new pane right of the document in front.
    OpenBeside(PathBuf),
    /// Open the file in a new pane below the document in front.
    OpenBelow(PathBuf),
    /// Open the drawing of an SVG file.
    OpenAsPreview(PathBuf),
    /// Open the page in a browser tab, served by the app.
    OpenInBrowserTab(PathBuf),
    /// Show the paths in the Finder, selected.
    RevealInFinder(Vec<PathBuf>),
    /// Put the text on the pasteboard.
    Copy(String),
    /// Move the paths to the Trash.
    MoveToTrash(Vec<PathBuf>),
    /// Ask for a name, prefilled with `current`, under `title`.
    AskName {
        /// What the name is for.
        naming: Naming,
        /// The question.
        title: &'static str,
        /// The text the field starts with.
        current: String,
    },
    /// Create the file or folder `path`.
    Create {
        /// Where it goes.
        path: PathBuf,
        /// A file or a folder.
        kind: Kind,
    },
    /// Rename `from` to `to`.
    Rename {
        /// The present path.
        from: PathBuf,
        /// The new path.
        to: PathBuf,
    },
    /// Tell the user that something did not work.
    Fail {
        /// What went wrong.
        message: String,
        /// Why, as the system said it.
        detail: String,
    },
}

/// The state of the files panel of one window.
#[derive(Debug)]
pub struct Model {
    root: PathBuf,
    rows: HashMap<PathBuf, Row>,
    /// The path the context menu was opened on.
    clicked: Option<PathBuf>,
    /// Every selected row when the menu opened on one of several selected rows, as in the Finder; empty otherwise.
    selection: Vec<PathBuf>,
}

impl Model {
    /// The panel of the folder `root`, with nothing read yet.
    pub fn new(root: PathBuf) -> Model {
        Model {
            root,
            rows: HashMap::new(),
            clicked: None,
            selection: Vec::new(),
        }
    }

    /// What the tree last read of `path`.
    pub fn row(&self, path: &Path) -> Option<Row> {
        self.rows.get(path).copied()
    }

    /// Records what the tree last read of `path`.
    pub fn note(&mut self, path: &Path, row: Row) {
        self.rows.insert(path.to_path_buf(), row);
    }

    /// Forgets what the tree read of the paths `live` does not know.
    pub fn keep_rows(&mut self, live: &Live) {
        self.rows.retain(|path, _| live.contains(path));
    }

    /// The row the context menu was opened on.
    pub fn clicked(&self) -> Option<&Path> {
        self.clicked.as_deref()
    }

    /// A single click opens a folder or closes it, as in VS Code, and opens a file, in the preview tab if the user
    /// turned those on.
    pub fn click(path: &Path, kind: Kind) -> Effect {
        match kind {
            Kind::Folder => Effect::Toggle,
            Kind::File => Effect::Preview(path.to_path_buf()),
        }
    }

    /// A double click opens a file in a tab of its own; a folder's first click already opened or closed it.
    pub fn double_click(path: &Path, kind: Kind) -> Option<Effect> {
        (kind == Kind::File).then(|| Effect::Open(path.to_path_buf()))
    }

    /// Remembers the row the context menu opens on, the root for the empty space below the rows, and the `selected`
    /// rows it acts on as well when it opens on one of them, and says what the menu offers; `kind` is what `path` is on
    /// disk, if it still is.
    pub fn open_menu(&mut self, path: Option<&Path>, kind: Option<Kind>, selected: &[PathBuf]) -> Vec<MenuEntry> {
        self.clicked = Some(path.map_or_else(|| self.root.clone(), Path::to_path_buf));
        let several = selected.len() > 1 && path.is_some_and(|path| selected.iter().any(|s| s == path));
        self.selection = if several { selected.to_vec() } else { Vec::new() };
        let file = kind == Some(Kind::File) && !several;
        let row = path.is_some();
        let item = |title, command, enabled| MenuEntry::Item {
            title,
            command,
            enabled,
        };
        let mut entries = vec![
            item(t("Daneben öffnen"), Command::OpenBeside, file),
            item(t("Darunter öffnen"), Command::OpenBelow, file),
            MenuEntry::Separator,
            item(t("Neue Datei …"), Command::NewFile, true),
            item(t("Neuer Ordner …"), Command::NewFolder, true),
            MenuEntry::Separator,
            item(t("Im Finder zeigen"), Command::Reveal, true),
            item(t("Pfad kopieren"), Command::CopyPath, true),
            item(t("Relativen Pfad kopieren"), Command::CopyRelativePath, true),
            MenuEntry::Separator,
            item(t("Umbenennen …"), Command::Rename, row && !several),
            item(t("In den Papierkorb legen"), Command::Trash, row),
        ];
        if file && path.is_some_and(is_page) {
            entries.insert(1, item(t("Im Browser-Tab öffnen"), Command::OpenInBrowserTab, true));
        }
        if file && path.is_some_and(is_svg) {
            entries.insert(1, item(t("Als Vorschau öffnen"), Command::OpenAsPreview, true));
        }
        entries
    }

    /// What `command` of the context menu does to the clicked row, or to every selected row for the commands that can
    /// act on several; `kind` is what the clicked row is on disk.
    pub fn command(&self, command: Command, kind: Option<Kind>) -> Option<Effect> {
        let path = self.clicked.clone()?;
        let targets = if self.selection.is_empty() {
            vec![path.clone()]
        } else {
            self.selection.clone()
        };
        Some(match command {
            Command::OpenBeside => Effect::OpenBeside(path),
            Command::OpenBelow => Effect::OpenBelow(path),
            Command::OpenAsPreview => Effect::OpenAsPreview(path),
            Command::OpenInBrowserTab => Effect::OpenInBrowserTab(path),
            Command::NewFile | Command::NewFolder => {
                let new = if command == Command::NewFolder {
                    Kind::Folder
                } else {
                    Kind::File
                };
                return Some(create_in(&path, kind, new));
            }
            Command::Reveal => Effect::RevealInFinder(targets),
            Command::CopyPath => Effect::Copy(lines(&targets, |p| p)),
            Command::CopyRelativePath => Effect::Copy(lines(&targets, |p| p.strip_prefix(&self.root).unwrap_or(p))),
            Command::Rename => Effect::AskName {
                current: file_name(&path),
                naming: Naming::Rename { path },
                title: t("Umbenennen"),
            },
            Command::Trash => Effect::MoveToTrash(outermost(targets)),
        })
    }

    /// Asks for the name of a new `new` where the title row's buttons create it: in the selected row's folder, the
    /// one a selected file is in, or the root without a selection.
    pub fn create_from_header(&self, selected: Option<&Path>, kind: Option<Kind>, new: Kind) -> Effect {
        match selected {
            Some(path) => create_in(path, kind, new),
            None => create_in(&self.root, Some(Kind::Folder), new),
        }
    }

    /// What follows the name the user typed for `naming`; `exists` says whether something has that name already.
    pub fn named(naming: &Naming, name: &str, exists: impl Fn(&Path) -> bool) -> Option<Effect> {
        match naming {
            Naming::Create { parent, kind } => Some(Effect::Create {
                path: parent.join(name),
                kind: *kind,
            }),
            Naming::Rename { path } => {
                let current = file_name(path);
                if name == current {
                    return None;
                }
                let target = path.with_file_name(name);
                if exists(&target) && !same_file_ignoring_case(path, &target) {
                    return Some(Effect::Fail {
                        message: tf("„{}“ gibt es schon.", &[name]),
                        detail: String::new(),
                    });
                }
                Some(Effect::Rename {
                    from: path.clone(),
                    to: target,
                })
            }
        }
    }

    /// What follows creating the `kind` at `path`: a new file opens, a failure is said.
    pub fn created(path: &Path, kind: Kind, result: io::Result<()>) -> Option<Effect> {
        match result {
            Ok(()) => (kind == Kind::File).then(|| Effect::Open(path.to_path_buf())),
            Err(error) => Some(Effect::Fail {
                message: tf("„{}“ wurde nicht angelegt.", &[&file_name(path)]),
                detail: error.explain(),
            }),
        }
    }

    /// What follows renaming `from`: only a failure is said.
    pub fn renamed(from: &Path, result: io::Result<()>) -> Option<Effect> {
        result.err().map(|error| Effect::Fail {
            message: tf("„{}“ wurde nicht umbenannt.", &[&file_name(from)]),
            detail: error.explain(),
        })
    }
}

/// The paths whose rows the outline view may still hold: the entries of the folders it was told, and anything in a
/// folder whose rows it has not read anew since the folder closed or while it waits for them.
pub struct Live<'a> {
    entries: HashSet<&'a Path>,
    unsettled: Vec<&'a Path>,
    /// The folder whose rows inside are in question; every path outside it counts as live.
    within: Option<&'a Path>,
}

impl<'a> Live<'a> {
    /// The rows of the folders `shown` with their entries, and of every path in the folders `unsettled`.
    pub fn new(
        shown: impl IntoIterator<Item = &'a [Entry]>,
        unsettled: impl IntoIterator<Item = &'a Path>,
    ) -> Live<'a> {
        Live {
            entries: shown.into_iter().flatten().map(|e| e.path.as_path()).collect(),
            unsettled: unsettled.into_iter().collect(),
            within: None,
        }
    }

    /// The rows inside `dir` only, of the folders `shown` there, `dir` itself among them, and the folders
    /// `unsettled`; whatever lies outside `dir` stays.
    pub fn within(
        dir: &'a Path,
        shown: impl IntoIterator<Item = (&'a Path, &'a [Entry])>,
        unsettled: impl IntoIterator<Item = &'a Path>,
    ) -> Live<'a> {
        let shown = shown
            .into_iter()
            .filter(|(folder, _)| *folder == dir || is_inside(folder, dir))
            .map(|(_, entries)| entries);
        Live {
            within: Some(dir),
            ..Live::new(shown, unsettled)
        }
    }

    /// Whether the outline view may still hold the row of `path`.
    pub fn contains(&self, path: &Path) -> bool {
        self.within.is_some_and(|dir| !is_inside(path, dir))
            || self.entries.contains(path)
            || self.unsettled.iter().any(|dir| path.starts_with(dir))
    }
}

/// Whether `path` lies inside `dir`, both as the tree joins them; a comparison of bytes, as it runs over every row.
pub fn is_inside(path: &Path, dir: &Path) -> bool {
    let (path, dir) = (path.as_os_str().as_encoded_bytes(), dir.as_os_str().as_encoded_bytes());
    let dir = dir.strip_suffix(b"/").unwrap_or(dir);
    path.len() > dir.len() + 1 && path.starts_with(dir) && path[dir.len()] == b'/'
}

/// The name the user typed into the field, or `None` if it is empty or holds a slash.
pub fn valid_name(typed: &str) -> Option<String> {
    let name = typed.trim();
    (!name.is_empty() && !name.contains('/')).then(|| name.to_string())
}

/// Asks for the name of a new `new` in `at`, or beside it if it is a file.
fn create_in(at: &Path, kind: Option<Kind>, new: Kind) -> Effect {
    let parent = if kind == Some(Kind::Folder) {
        at.to_path_buf()
    } else {
        at.parent().map_or_else(|| at.to_path_buf(), Path::to_path_buf)
    };
    Effect::AskName {
        naming: Naming::Create { parent, kind: new },
        title: match new {
            Kind::Folder => t("Neuer Ordner"),
            Kind::File => t("Neue Datei"),
        },
        current: String::new(),
    }
}

/// `paths` one per line, each as `shown` puts it.
fn lines<'a>(paths: &'a [PathBuf], shown: impl Fn(&'a Path) -> &'a Path) -> String {
    paths
        .iter()
        .map(|path| shown(path).to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `paths` without those inside another of them, which go along with it.
fn outermost(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let inside = |path: &PathBuf| paths.iter().any(|other| other != path && path.starts_with(other));
    paths.iter().filter(|path| !inside(path)).cloned().collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `a` and `b` name one file on a volume that ignores case, as a rename that only changes case does.
fn same_file_ignoring_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Whether `path` is an HTML page.
fn is_page(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

/// Whether `path` names an SVG drawing, which opens as a picture as well as as text.
pub(crate) fn is_svg(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

impl Row {
    /// What the tree shows of `entry` besides its path.
    pub fn of(entry: &Entry) -> Row {
        Row {
            expandable: entry.can_expand(),
            ignored: entry.is_ignored || entry.is_hidden,
        }
    }
}

/// How the rows of a folder change from one reading to the next, so the tree moves only those.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The positions in the old list of the rows that go, removed first.
    pub removed: Vec<usize>,
    /// The positions in the new list of the rows that come, inserted after the removals.
    pub inserted: Vec<usize>,
    /// The positions in the new list of the rows that stay but look different.
    pub redrawn: Vec<usize>,
}

/// The changes from the rows `old` to `new`; `None` where removing and inserting cannot get there, such as when a
/// row turns from a file into a folder or rows swap places, and the folder's rows are shown again as a whole.
pub fn changes(old: &[Entry], new: &[Entry]) -> Option<Changes> {
    let before: HashMap<&Path, usize> = old.iter().enumerate().map(|(i, e)| (e.path.as_path(), i)).collect();
    let after: HashMap<&Path, usize> = new.iter().enumerate().map(|(i, e)| (e.path.as_path(), i)).collect();
    let mut changes = Changes {
        removed: (0..old.len())
            .filter(|i| !after.contains_key(old[*i].path.as_path()))
            .collect(),
        ..Changes::default()
    };
    let mut last = None;
    for (i, entry) in new.iter().enumerate() {
        let Some(&was) = before.get(entry.path.as_path()) else {
            changes.inserted.push(i);
            continue;
        };
        if last.is_some_and(|last| was < last) || Row::of(&old[was]).expandable != Row::of(entry).expandable {
            return None;
        }
        last = Some(was);
        if Row::of(&old[was]) != Row::of(entry) {
            changes.redrawn.push(i);
        }
    }
    Some(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Kind as EntryKind;

    fn entry(name: &str, kind: EntryKind, is_ignored: bool) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/p").join(name),
            kind,
            link: None,
            size: 0,
            modified: None,
            is_ignored,
            is_hidden: false,
        }
    }

    #[test]
    fn a_folder_read_again_moves_only_the_rows_that_came_went_or_look_different() {
        let old = [
            entry("a", EntryKind::Dir, false),
            entry("b.rs", EntryKind::File, false),
            entry("c.rs", EntryKind::File, false),
        ];
        let new = [
            entry("a", EntryKind::Dir, false),
            entry("a.rs", EntryKind::File, false),
            entry("c.rs", EntryKind::File, true),
            entry("d.rs", EntryKind::File, false),
        ];
        assert_eq!(
            changes(&old, &new),
            Some(Changes {
                removed: vec![1],
                inserted: vec![1, 3],
                redrawn: vec![2],
            })
        );
        assert_eq!(changes(&new, &new), Some(Changes::default()));
    }

    #[test]
    fn rows_live_while_their_folder_lists_them_or_has_not_settled() {
        let top = [entry("a", EntryKind::Dir, false), entry("b.rs", EntryKind::File, false)];
        let inside = [Entry {
            path: "/p/a/c.rs".into(),
            ..entry("c.rs", EntryKind::File, false)
        }];
        let waiting = Path::new("/p/w");
        let live = Live::new([&top[..], &inside[..]], [waiting]);
        for path in ["/p/a", "/p/b.rs", "/p/a/c.rs", "/p/w", "/p/w/x/y.rs"] {
            assert!(live.contains(Path::new(path)), "{path}");
        }
        for path in ["/p/gone.rs", "/p/a/d.rs", "/p/wx"] {
            assert!(!live.contains(Path::new(path)), "{path}");
        }
        let mut model = model();
        model.note(Path::new("/p/b.rs"), Row::of(&top[1]));
        model.note(Path::new("/p/gone.rs"), Row::default());
        model.keep_rows(&live);
        assert!(model.row(Path::new("/p/b.rs")).is_some());
        assert_eq!(model.row(Path::new("/p/gone.rs")), None);
    }

    #[test]
    fn rows_inside_a_folder_live_while_a_folder_there_lists_them_and_all_outside_it_stays() {
        let inside = [Entry {
            path: "/p/a/c.rs".into(),
            ..entry("c.rs", EntryKind::File, false)
        }];
        let elsewhere = [entry("b.rs", EntryKind::File, false)];
        let (a, p) = (Path::new("/p/a"), Path::new("/p"));
        let live = Live::within(a, [(a, &inside[..]), (p, &elsewhere[..])], []);
        for path in ["/p/a", "/p/a/c.rs", "/p/gone.rs", "/p/ab/x.rs", "/q"] {
            assert!(live.contains(Path::new(path)), "{path}");
        }
        for path in ["/p/a/d.rs", "/p/a/c.rs/x"] {
            assert!(!live.contains(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn inside_means_below_a_folder_not_beside_it() {
        assert!(is_inside(Path::new("/p/a/b"), Path::new("/p/a")));
        assert!(is_inside(Path::new("/p/a/b"), Path::new("/")));
        assert!(!is_inside(Path::new("/p/a"), Path::new("/p/a")));
        assert!(!is_inside(Path::new("/p/ab"), Path::new("/p/a")));
        assert!(!is_inside(Path::new("/p"), Path::new("/p/a")));
    }

    #[test]
    fn rows_that_swap_places_or_a_file_that_turns_into_a_folder_show_the_folder_again() {
        let a = entry("a", EntryKind::File, false);
        let b = entry("b", EntryKind::File, false);
        assert_eq!(changes(&[a.clone(), b.clone()], &[b, a.clone()]), None);
        assert_eq!(changes(&[a], &[entry("a", EntryKind::Dir, false)]), None);
    }

    fn model() -> Model {
        Model::new(PathBuf::from("/p"))
    }

    fn enabled(entries: &[MenuEntry], wanted: Command) -> Option<bool> {
        entries.iter().find_map(|entry| match entry {
            MenuEntry::Item { command, enabled, .. } if *command == wanted => Some(*enabled),
            _ => None,
        })
    }

    #[test]
    fn a_click_opens_a_folder_or_previews_a_file_and_a_double_click_opens_only_files() {
        let path = Path::new("/p/a");
        assert_eq!(Model::click(path, Kind::Folder), Effect::Toggle);
        assert_eq!(Model::click(path, Kind::File), Effect::Preview(path.into()));
        assert_eq!(Model::double_click(path, Kind::Folder), None);
        assert_eq!(Model::double_click(path, Kind::File), Some(Effect::Open(path.into())));
    }

    #[test]
    fn the_menu_on_empty_space_acts_on_the_root_and_cannot_rename_it() {
        let mut model = model();
        let entries = model.open_menu(None, None, &[]);
        assert_eq!(model.clicked(), Some(Path::new("/p")));
        assert_eq!(enabled(&entries, Command::Rename), Some(false));
        assert_eq!(enabled(&entries, Command::Trash), Some(false));
        assert_eq!(enabled(&entries, Command::OpenBeside), Some(false));
        assert_eq!(enabled(&entries, Command::NewFile), Some(true));
        assert_eq!(enabled(&entries, Command::OpenAsPreview), None);
    }

    #[test]
    fn only_an_svg_file_offers_its_drawing() {
        let mut model = model();
        let entries = model.open_menu(Some(Path::new("/p/logo.SVG")), Some(Kind::File), &[]);
        assert_eq!(enabled(&entries, Command::OpenAsPreview), Some(true));
        assert_eq!(enabled(&entries, Command::OpenBeside), Some(true));
        let entries = model.open_menu(Some(Path::new("/p/x.svg")), Some(Kind::Folder), &[]);
        assert_eq!(enabled(&entries, Command::OpenAsPreview), None);
    }

    #[test]
    fn a_new_file_goes_into_a_clicked_folder_or_beside_a_clicked_file() {
        let mut model = model();
        model.open_menu(Some(Path::new("/p/src")), Some(Kind::Folder), &[]);
        let Some(Effect::AskName { naming, .. }) = model.command(Command::NewFile, Some(Kind::Folder)) else {
            panic!("no question for the name");
        };
        assert_eq!(
            naming,
            Naming::Create {
                parent: "/p/src".into(),
                kind: Kind::File
            }
        );
        model.open_menu(Some(Path::new("/p/src/main.rs")), Some(Kind::File), &[]);
        let Some(Effect::AskName { naming, .. }) = model.command(Command::NewFolder, Some(Kind::File)) else {
            panic!("no question for the name");
        };
        assert_eq!(
            naming,
            Naming::Create {
                parent: "/p/src".into(),
                kind: Kind::Folder
            }
        );
        assert_eq!(
            Model::named(&naming, "lib", |_| false),
            Some(Effect::Create {
                path: "/p/src/lib".into(),
                kind: Kind::Folder
            })
        );
    }

    #[test]
    fn the_header_creates_in_the_root_without_a_selection() {
        let Effect::AskName { naming, .. } = model().create_from_header(None, None, Kind::File) else {
            panic!("no question for the name");
        };
        assert_eq!(
            naming,
            Naming::Create {
                parent: "/p".into(),
                kind: Kind::File
            }
        );
    }

    #[test]
    fn copying_the_relative_path_leaves_out_the_root() {
        let mut model = model();
        model.open_menu(Some(Path::new("/p/src/main.rs")), Some(Kind::File), &[]);
        assert_eq!(
            model.command(Command::CopyRelativePath, Some(Kind::File)),
            Some(Effect::Copy("src/main.rs".into()))
        );
        assert_eq!(
            model.command(Command::CopyPath, Some(Kind::File)),
            Some(Effect::Copy("/p/src/main.rs".into()))
        );
    }

    #[test]
    fn the_menu_on_one_of_several_selected_rows_acts_on_them_all_and_offers_no_one_file_commands() {
        let mut model = model();
        let selected = [
            PathBuf::from("/p/src"),
            PathBuf::from("/p/src/lib.rs"),
            PathBuf::from("/p/notes.md"),
        ];
        let entries = model.open_menu(Some(Path::new("/p/notes.md")), Some(Kind::File), &selected);
        assert_eq!(enabled(&entries, Command::Rename), Some(false));
        assert_eq!(enabled(&entries, Command::OpenBeside), Some(false));
        assert_eq!(enabled(&entries, Command::Trash), Some(true));
        assert_eq!(
            model.command(Command::CopyRelativePath, Some(Kind::File)),
            Some(Effect::Copy("src\nsrc/lib.rs\nnotes.md".into()))
        );
        assert_eq!(
            model.command(Command::Trash, Some(Kind::File)),
            Some(Effect::MoveToTrash(vec!["/p/src".into(), "/p/notes.md".into()]))
        );
        assert!(matches!(
            model.command(Command::NewFile, Some(Kind::File)),
            Some(Effect::AskName { naming: Naming::Create { parent, .. }, .. }) if parent == Path::new("/p")
        ));
    }

    #[test]
    fn the_menu_on_a_row_outside_the_selection_acts_on_that_row_alone() {
        let mut model = model();
        let selected = [PathBuf::from("/p/a.rs"), PathBuf::from("/p/b.rs")];
        let entries = model.open_menu(Some(Path::new("/p/c.rs")), Some(Kind::File), &selected);
        assert_eq!(enabled(&entries, Command::Rename), Some(true));
        assert_eq!(
            model.command(Command::Reveal, Some(Kind::File)),
            Some(Effect::RevealInFinder(vec!["/p/c.rs".into()]))
        );
        model.open_menu(Some(Path::new("/p/a.rs")), Some(Kind::File), &selected[..1]);
        assert_eq!(
            model.command(Command::Trash, Some(Kind::File)),
            Some(Effect::MoveToTrash(vec!["/p/a.rs".into()]))
        );
    }

    #[test]
    fn a_rename_keeps_out_of_the_way_of_another_file_but_may_change_case() {
        let naming = Naming::Rename {
            path: "/p/Readme.md".into(),
        };
        assert_eq!(Model::named(&naming, "Readme.md", |_| true), None);
        assert!(matches!(
            Model::named(&naming, "other.md", |_| true),
            Some(Effect::Fail { .. })
        ));
        assert_eq!(
            Model::named(&naming, "README.md", |_| true),
            Some(Effect::Rename {
                from: "/p/Readme.md".into(),
                to: "/p/README.md".into()
            })
        );
    }

    #[test]
    fn a_created_file_opens_and_a_failure_is_said() {
        let path = Path::new("/p/a.rs");
        assert_eq!(
            Model::created(path, Kind::File, Ok(())),
            Some(Effect::Open(path.into()))
        );
        assert_eq!(Model::created(path, Kind::Folder, Ok(())), None);
        let exists = io::Error::from(io::ErrorKind::AlreadyExists);
        let said = exists.explain();
        assert!(matches!(
            Model::created(path, Kind::File, Err(exists)),
            Some(Effect::Fail { detail, .. }) if detail == said
        ));
        assert_eq!(Model::renamed(path, Ok(())), None);
    }

    #[test]
    fn a_name_is_trimmed_and_may_hold_no_slash() {
        assert_eq!(valid_name("  a.rs "), Some("a.rs".into()));
        assert_eq!(valid_name("   "), None);
        assert_eq!(valid_name("a/b"), None);
    }

    #[test]
    fn svg_files_are_known_by_extension_in_any_case() {
        assert!(is_svg(Path::new("logo.Svg")));
        assert!(!is_svg(Path::new("logo.svg.txt")));
    }
}
