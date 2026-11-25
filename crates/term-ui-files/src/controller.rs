//! The controller of the files panel: hands the view's events to the model and carries out the effects it returns, on
//! the file system, the pasteboard, in the Finder and in the window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSOutlineView, NSPasteboard, NSPasteboardTypeString, NSTableViewAnimationOptions, NSTextField, NSWorkspace,
};
use objc2_foundation::{NSArray, NSFileManager, NSMutableIndexSet, NSPoint, NSRect, NSSize, NSURL};
use splat_i18n::t;
use term_project::Entry;
use term_ui::kit::alert::{Alert, Role, first, warn};

use term_project::files_panel::{
    Changes, Command, Effect, Kind, Live, MenuEntry, Model, Naming, changes, is_inside, valid_name,
};
use term_render::constant;
use term_ui::kit::{FlagGuard, main_after, ns};

use crate::view::{FileTreeSource, Pending, path_of};

/// How long a folder's entries may take to read before the folder shows a spinner.
const SPINNER_AFTER: Duration = Duration::from_millis(100);

impl FileTreeSource {
    pub(crate) fn row_clicked(&self, view: &NSOutlineView, row: isize) {
        let Some(item) = (row >= 0).then(|| view.itemAtRow(row)).flatten() else {
            return;
        };
        let Some(path) = path_of(&item) else { return };
        let kind = if path.is_dir() { Kind::Folder } else { Kind::File };
        match Model::click(&path, kind) {
            // SAFETY: The item came from this outline view, which knows it.
            Effect::Toggle => unsafe {
                if view.isItemExpanded(Some(&item)) {
                    view.collapseItem(Some(&item));
                } else {
                    view.expandItem(Some(&item));
                }
            },
            effect => self.run(effect),
        }
    }

    pub(crate) fn row_double_clicked(&self, view: &NSOutlineView, row: isize) {
        let Some(path) = (row >= 0)
            .then(|| view.itemAtRow(row))
            .flatten()
            .and_then(|item| path_of(&item))
        else {
            return;
        };
        let kind = if path.is_dir() { Kind::Folder } else { Kind::File };
        if let Some(effect) = Model::double_click(&path, kind) {
            self.run(effect);
        }
    }

    /// What the context menu offers for `path`, the row it opens on, with the `selected` rows.
    pub(crate) fn menu_opens(&self, path: Option<&Path>, selected: &[PathBuf]) -> Vec<MenuEntry> {
        let kind = path.and_then(kind_of);
        self.ivars().model.borrow_mut().open_menu(path, kind, selected)
    }

    pub(crate) fn command(&self, command: Command) {
        let effect = {
            let model = self.ivars().model.borrow();
            let kind = model.clicked().and_then(kind_of);
            model.command(command, kind)
        };
        if let Some(effect) = effect {
            self.run(effect);
        }
    }

    pub(crate) fn create_from_header(&self, new: Kind) {
        let selected = self
            .outline()
            .and_then(|view| view.itemAtRow(view.selectedRow()))
            .and_then(|item| path_of(&item));
        let kind = selected.as_deref().and_then(kind_of);
        let effect = self
            .ivars()
            .model
            .borrow()
            .create_from_header(selected.as_deref(), kind, new);
        self.run(effect);
    }

    /// The folder `path` opens: its entries are read on the disk lane, with a spinner in the folder once that takes
    /// `SPINNER_AFTER`; `deep` opens the folders inside as well once they are known.
    pub(crate) fn folder_opens(&self, path: &Path, deep: bool) {
        let ivars = self.ivars();
        if ivars.shown.borrow().contains_key(path) {
            return;
        }
        let known = ivars
            .pending
            .borrow_mut()
            .insert(path.to_path_buf(), Pending { spinner: None, deep })
            .is_some();
        if known {
            return;
        }
        ivars.project.expand(path);
        self.spin_later(path.to_path_buf());
    }

    /// Waits `SPINNER_AFTER`, then shows a spinner in `dir` if its entries are still being read.
    pub(crate) fn spin_later(&self, dir: PathBuf) {
        let panel = self.ivars().id;
        main_after(SPINNER_AFTER, move || {
            crate::panel::with(panel, |source| {
                if let Some(view) = source.outline() {
                    source.spin(&view, &dir);
                }
            });
        });
    }

    pub(crate) fn folder_closed(&self, path: &Path) {
        let ivars = self.ivars();
        ivars.shown.borrow_mut().retain(|dir, _| !dir.starts_with(path));
        ivars.pending.borrow_mut().retain(|dir, _| !dir.starts_with(path));
        ivars.project.collapse(path);
        let first = {
            let mut closing = ivars.closing.borrow_mut();
            closing.push(path.to_path_buf());
            closing.len() == 1
        };
        // Reloading inside the collapse notification could change rows the outline view is still collapsing.
        if first {
            let panel = ivars.id;
            term_ui::kit::main_soon(move || {
                crate::panel::with(panel, |source| {
                    if let Some(view) = source.outline() {
                        source.settle(&view);
                    }
                });
            });
        }
    }

    /// Has the outline view read the folders that closed and are still closed anew, which leaves them without rows
    /// inside, then lets go of the rows it no longer holds.
    pub(crate) fn settle(&self, view: &NSOutlineView) {
        let closing = std::mem::take(&mut *self.ivars().closing.borrow_mut());
        for dir in &closing {
            let Some(item) = self.item_of(dir) else { continue };
            // SAFETY: The item is kept alive in `items`; the view answers -1 and false for one it does not know.
            let (row, open) = unsafe { (view.rowForItem(Some(&item)), view.isItemExpanded(Some(&item))) };
            if row >= 0 && !open {
                // SAFETY: The item is one the data source handed this view, which shows its row.
                unsafe { view.reloadItem_reloadChildren(Some(&item), true) };
            }
        }
        self.ivars().stale.borrow_mut().extend(closing);
        self.forget_rows();
    }

    /// Lets go of the items and rows of paths the outline view no longer holds, once rows left it; only inside the
    /// folders whose rows changed, as a big tree holds many thousand.
    pub(crate) fn forget_rows(&self) {
        let ivars = self.ivars();
        let stale = outermost(std::mem::take(&mut *ivars.stale.borrow_mut()));
        if stale.is_empty() {
            return;
        }
        let (shown, pending, closing) = (ivars.shown.borrow(), ivars.pending.borrow(), ivars.closing.borrow());
        for dir in &stale {
            let unsettled = pending.keys().chain(closing.iter()).map(PathBuf::as_path);
            let folders = shown.iter().map(|(folder, entries)| (folder.as_path(), &entries[..]));
            let live = Live::within(dir, folders, unsettled);
            ivars.items.keep(|path| live.contains(path));
            ivars.model.borrow_mut().keep_rows(&live);
        }
    }

    /// Shows a spinner in `dir` while its entries are still being read.
    pub(crate) fn spin(&self, view: &NSOutlineView, dir: &Path) {
        {
            let mut pending = self.ivars().pending.borrow_mut();
            let Some(waiting) = pending.get_mut(dir).filter(|p| p.spinner.is_none()) else {
                return;
            };
            waiting.spinner = Some(NSObject::new());
        }
        self.reload_folder(view, dir);
    }

    /// Shows what the tree read of `dir`: all its rows where it just opened, else only the rows that came, went or
    /// look different, which leaves the folders inside as they are.
    pub(crate) fn listed(&self, view: &NSOutlineView, dir: &Path, entries: Option<&Arc<[Entry]>>) {
        let ivars = self.ivars();
        let waiting = ivars.pending.borrow_mut().remove(dir);
        let old = ivars.shown.borrow().get(dir).cloned();
        let new = entries.cloned().unwrap_or_else(|| Arc::from([]));
        match (old, &waiting) {
            (Some(old), None) => {
                let changes = changes(&old, &new);
                ivars.shown.borrow_mut().insert(dir.to_path_buf(), new.clone());
                match changes {
                    Some(changes) => self.move_rows(view, dir, &new, &changes),
                    None => self.reload_folder(view, dir),
                }
            }
            (_, Some(waiting)) => {
                ivars.shown.borrow_mut().insert(dir.to_path_buf(), new.clone());
                self.reload_folder(view, dir);
                if waiting.deep {
                    self.open_inside(view, &new);
                }
            }
            (None, None) => return,
        }
        if entries.is_none() {
            ivars.shown.borrow_mut().remove(dir);
        }
    }

    fn move_rows(&self, view: &NSOutlineView, dir: &Path, entries: &[Entry], changes: &Changes) {
        let parent = self.item_of(dir);
        if parent.is_none() && dir != self.ivars().project.root() {
            return;
        }
        let parent: Option<&AnyObject> = parent.as_deref().map(|p| p.as_ref());
        let set = |indexes: &[usize]| {
            let set = NSMutableIndexSet::new();
            for index in indexes {
                set.addIndex(*index);
            }
            set
        };
        let none = NSTableViewAnimationOptions::empty();
        view.beginUpdates();
        // SAFETY: The parent is nil for the root or an item the data source handed this view, and the indexes are
        // those of the rows it showed before and shows now.
        unsafe {
            if !changes.removed.is_empty() {
                view.removeItemsAtIndexes_inParent_withAnimation(&set(&changes.removed), parent, none);
            }
            if !changes.inserted.is_empty() {
                view.insertItemsAtIndexes_inParent_withAnimation(&set(&changes.inserted), parent, none);
            }
        }
        view.endUpdates();
        if !changes.removed.is_empty() {
            self.ivars().stale.borrow_mut().push(dir.to_path_buf());
        }
        for entry in changes.redrawn.iter().filter_map(|i| entries.get(*i)) {
            let item = self.item_for(entry);
            // SAFETY: The item is one the data source handed this view.
            unsafe { view.reloadItem(Some(&item)) };
        }
    }

    /// Opens the folders among `entries`, and the folders inside them as they are read.
    fn open_inside(&self, view: &NSOutlineView, entries: &[Entry]) {
        let _cascade = FlagGuard::raise(&self.ivars().cascade);
        for entry in entries.iter().filter(|e| e.can_expand()) {
            let item = self.item_for(entry);
            // SAFETY: The item is one the data source handed this view for a row of the folder just shown.
            unsafe { view.expandItem_expandChildren(Some(&item), true) };
        }
    }

    /// Asks the view for the rows of `dir` again, and of the folders inside.
    fn reload_folder(&self, view: &NSOutlineView, dir: &Path) {
        self.ivars().stale.borrow_mut().push(dir.to_path_buf());
        if dir == self.ivars().project.root() {
            // SAFETY: Nil stands for the root, whose children the data source answers for.
            unsafe { view.reloadItem_reloadChildren(None, true) };
        } else if let Some(item) = self.item_of(dir) {
            // SAFETY: The item is one the data source handed to this view, kept alive by the source.
            unsafe { view.reloadItem_reloadChildren(Some(&item), true) };
        }
    }

    fn run(&self, effect: Effect) {
        let workspace = self.ivars().workspace;
        let host = crate::host::installed();
        match effect {
            Effect::Toggle => {}
            Effect::Preview(path) => open(workspace, &path, true),
            Effect::Open(path) | Effect::OpenBeside(path) | Effect::OpenBelow(path) => open(workspace, &path, false),
            Effect::OpenAsPreview(path) => {
                if let Some(host) = host {
                    host.open_as_preview(workspace, &path);
                }
            }
            Effect::OpenInBrowserTab(path) => {
                if let Some(host) = host {
                    host.open_in_browser_tab(workspace, &path);
                }
            }
            Effect::RevealInFinder(paths) => {
                let urls: Vec<_> = paths.iter().map(|path| file_url(path)).collect();
                NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&urls));
            }
            Effect::Copy(text) => copy_text(&text),
            Effect::MoveToTrash(paths) => {
                for path in paths {
                    let moved =
                        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&file_url(&path), None);
                    if let Err(error) = moved {
                        term_base::diag::report("moving to the Trash", error.localizedDescription());
                    }
                }
            }
            Effect::AskName { naming, title, current } => self.ask_name(&naming, title, &current),
            Effect::Create { path, kind } => {
                let result = create(&path, kind);
                if let Some(effect) = Model::created(&path, kind, result) {
                    self.run(effect);
                }
            }
            Effect::Rename { from, to } => {
                let result = std::fs::rename(&from, &to);
                if let Some(effect) = Model::renamed(&from, result) {
                    self.run(effect);
                }
            }
            Effect::Fail { message, detail } => warn(self.mtm(), &message, &detail),
        }
    }

    fn ask_name(&self, naming: &Naming, title: &str, current: &str) {
        let Some(name) = ask_name(self.mtm(), title, current) else {
            return;
        };
        if let Some(effect) = Model::named(naming, &name, Path::exists) {
            self.run(effect);
        }
    }
}

/// Opens `path` in a tab of `workspace` as the host does, or in an editor tab without one.
fn open(workspace: term_ui::workspace_host::WorkspaceId, path: &Path, preview: bool) {
    match crate::host::installed() {
        Some(host) => host.open_file(workspace, path, preview),
        None => term_ui_editor::open_tab(&term_ui::workspace_host::Workspace::new(workspace), path, preview),
    }
}

/// What `path` is on disk, `None` if nothing is there.
fn kind_of(path: &Path) -> Option<Kind> {
    if path.is_dir() {
        Some(Kind::Folder)
    } else if path.is_file() {
        Some(Kind::File)
    } else {
        None
    }
}

/// `dirs` without those inside another of them, once each.
fn outermost(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort();
    dirs.dedup();
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        // Sorted by component, a folder comes right before what lies inside it.
        if out.last().is_none_or(|last| !is_inside(&dir, last)) {
            out.push(dir);
        }
    }
    out
}

fn create(path: &Path, kind: Kind) -> std::io::Result<()> {
    match kind {
        Kind::Folder => std::fs::create_dir(path),
        Kind::File => std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map(|_| ()),
    }
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))
}

fn copy_text(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(&ns(text), constant!(NSPasteboardTypeString));
}

/// Asks for a file name; `None` if the user cancels or leaves it empty, or it holds a slash.
fn ask_name(mtm: MainThreadMarker, title: &str, current: &str) -> Option<String> {
    let field = NSTextField::textFieldWithString(&ns(current), mtm);
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)));
    let response = Alert::new(mtm, title)
        .button(t("OK"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&field)
        .focus(&field)
        .run_modal();
    if !first(response) {
        return None;
    }
    valid_name(&field.stringValue().to_string())
}

#[cfg(test)]
mod tests {
    use term_base::testing::Scratch;

    use super::*;

    #[test]
    fn a_new_file_or_folder_is_made_and_then_known_as_such() {
        let scratch = Scratch::new("files-create");
        let file = scratch.path().join("a.txt");
        let folder = scratch.path().join("sub");
        assert_eq!(kind_of(&file), None);
        create(&file, Kind::File).unwrap();
        create(&folder, Kind::Folder).unwrap();
        assert_eq!(kind_of(&file), Some(Kind::File));
        assert_eq!(kind_of(&folder), Some(Kind::Folder));
    }

    #[test]
    fn of_folders_inside_one_another_only_the_outer_one_stays() {
        let dirs = ["/p/a/b", "/p/c", "/p/a", "/p/ab", "/p/a"].map(PathBuf::from).to_vec();
        assert_eq!(outermost(dirs), ["/p/a", "/p/ab", "/p/c"].map(PathBuf::from).to_vec());
    }

    #[test]
    fn creating_never_overwrites_what_is_there() {
        let scratch = Scratch::new("files-exists");
        let file = scratch.path().join("a.txt");
        std::fs::write(&file, "kept").unwrap();
        assert!(create(&file, Kind::File).is_err());
        assert!(create(scratch.path(), Kind::Folder).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
    }
}
