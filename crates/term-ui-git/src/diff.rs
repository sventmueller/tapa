//! The diff tab: one file's staged or unstaged changes, read only, with buttons that stage, unstage or discard a single
//! hunk; or, without buttons, what a commit of the history or an entry of the stash changed in it. It opens as a tab
//! of the workspace, as wide as its content, since the panel in the sidebar has no room for it; a single click in the
//! panel shows it in one preview tab that the next single click replaces, a double click keeps it.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSPasteboard, NSPasteboardTypeString, NSTableColumn, NSTableRowView,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSView,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_git::{Change, CommitId, DiffSource, Error, FileDiff, FileState, LineKind, Policy, StatusOptions};

use super::diff_view::{self, Row, Ui};
use super::text::{Problem, problem};
use super::{sheets, work};
use term_render::constant;
use term_ui::kit::{guarded, ns};
use term_ui::workspace_host::{AsWorkspace, Files, TabId, TabKind, Tabs, Workspace, WorkspaceId};

/// What a hunk button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HunkAction {
    Stage,
    Unstage,
    Discard,
}

/// A change that is done and only shows, never staged or discarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fixed {
    /// What a commit changed.
    Commit(CommitId),
    /// What a stash entry changed: its commit and its name, such as `stash@{0}`.
    Stash(CommitId, String),
}

#[derive(Default)]
pub(crate) struct State {
    pub(crate) diff: Option<FileDiff>,
    pub(crate) rows: Vec<Row>,
    loading: bool,
    again: bool,
    busy: bool,
    loaded: bool,
}

pub(crate) struct DiffIvars {
    id: u64,
    controller: Workspace,
    /// The workspace's base path, whose Git panel learns of changes made here.
    window_root: PathBuf,
    repo_root: PathBuf,
    /// The file, relative to the repository's root.
    path: PathBuf,
    staged: bool,
    /// The commit or stash entry whose change of the file shows.
    commit: Option<Fixed>,
    ui: OnceCell<Ui>,
    state: RefCell<State>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `DiffTab` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitDiffTab"]
    #[ivars = DiffIvars]
    pub(crate) struct DiffTab;

    unsafe impl NSObjectProtocol for DiffTab {}

    unsafe impl NSTableViewDataSource for DiffTab {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn row_count(&self, _table: &NSTableView) -> isize {
            guarded(0, || isize::try_from(self.ivars().state.borrow().rows.len()).unwrap_or(0))
        }
    }

    unsafe impl NSControlTextEditingDelegate for DiffTab {}

    unsafe impl NSTableViewDelegate for DiffTab {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, table: &NSTableView, column: Option<&NSTableColumn>, row: isize) -> Option<Retained<NSView>> {
            guarded(None, || {
                let state = self.ivars().state.borrow();
                let entry = *state.rows.get(usize::try_from(row).ok()?)?;
                let diff = state.diff.as_ref()?;
                let ui = self.ivars().ui.get()?;
                match (entry, column) {
                    (Row::Hunk(index), _) => Some(diff_view::hunk_header(
                        self,
                        diff,
                        index,
                        self.ivars().staged,
                        self.ivars().commit.is_some(),
                    )),
                    (Row::Line(hunk, line), Some(column)) => {
                        let line = diff.hunks.get(hunk)?.lines.get(line)?;
                        Some(ui.line_cell(table, &column.identifier().to_string(), line))
                    }
                    (Row::Line(..), None) => None,
                }
            })
        }

        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group_row(&self, _table: &NSTableView, row: isize) -> bool {
            guarded(false, || matches!(self.row(row), Some(Row::Hunk(_))))
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of_row(&self, _table: &NSTableView, row: isize) -> f64 {
            guarded(20.0, || {
                let line = self.ivars().ui.get().map_or(18.0, Ui::line_height);
                match self.row(row) {
                    Some(Row::Hunk(_)) => diff_view::HUNK_HEIGHT,
                    _ => line,
                }
            })
        }

        #[unsafe(method(tableView:didAddRowView:forRow:))]
        fn did_add_row_view(&self, _table: &NSTableView, view: &NSTableRowView, row: isize) {
            guarded((), || {
                let kind = self.line_kind(row);
                if let Some(color) = kind.and_then(diff_view::background) {
                    view.setBackgroundColor(&color);
                }
            });
        }
    }

    impl DiffTab {
        #[unsafe(method(termDiffStageHunk:))]
        fn stage_hunk_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.hunk_action(HunkAction::Stage, tag(sender)));
        }

        #[unsafe(method(termDiffUnstageHunk:))]
        fn unstage_hunk_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.hunk_action(HunkAction::Unstage, tag(sender)));
        }

        #[unsafe(method(termDiffDiscardHunk:))]
        fn discard_hunk_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.hunk_action(HunkAction::Discard, tag(sender)));
        }

        #[unsafe(method(termDiffOpenFile:))]
        fn open_file_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                let full = self.ivars().repo_root.join(&self.ivars().path);
                if let Some(controller) = Workspace::open(self.ivars().controller.id())
                    && full.is_file()
                {
                    controller.open_file(&full);
                }
            });
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.copy_selection());
        }
    }
);

fn with_tab(id: u64, f: impl FnOnce(&DiffTab)) {
    crate::host::state().diff_tabs.with(id, f);
}

fn live_tabs() -> Vec<Retained<DiffTab>> {
    crate::host::state().diff_tabs.live()
}

/// Reads the diffs of the tabs of the workspace whose base path is `window_root` again.
pub(crate) fn refresh_for(window_root: &Path) {
    for tab in live_tabs() {
        if tab.ivars().window_root == window_root {
            tab.reload();
        }
    }
}

/// The key of the tab that shows `path`'s staged or unstaged changes.
pub(crate) fn key(path: &Path, staged: bool) -> String {
    format!("diff:{}:{}", if staged { "staged" } else { "unstaged" }, path.display())
}

/// Shows the diff of `path`, relative to `repo_root`, in its tab, opening one if there is none; `preview` opens it
/// in the workspace's preview tab and leaves an open tab as it is, else an open preview tab is kept.
pub(crate) fn show(
    controller: &(impl AsWorkspace + ?Sized),
    repo_root: &Path,
    path: &Path,
    staged: bool,
    preview: bool,
) {
    let controller = &controller.as_workspace();
    let key = key(path, staged);
    if select_open(controller, &key, preview) {
        return;
    }
    let Some(window_root) = controller.project_root() else {
        return;
    };
    let window_root = window_root.canonicalize().unwrap_or(window_root);
    let tab = DiffTab::new(
        controller,
        window_root,
        repo_root.to_path_buf(),
        path.to_path_buf(),
        staged,
        None,
    );
    let name = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let title = if staged {
        tf("{} (vorgemerkt)", &[&name])
    } else {
        tf("{} (Änderungen)", &[&name])
    };
    place(controller, tab, key, &title, preview);
}

/// The key of the tab that shows what the commit `hash` changed in `path`.
pub(crate) fn commit_key(hash: &CommitId, path: &Path) -> String {
    format!("diff:commit:{hash}:{}", path.display())
}

/// Shows what the commit `hash` changed in `path`, relative to `repo_root`, like [`show`].
pub(crate) fn show_commit(
    controller: &(impl AsWorkspace + ?Sized),
    repo_root: &Path,
    hash: &CommitId,
    path: &Path,
    preview: bool,
) {
    let controller = &controller.as_workspace();
    let key = commit_key(hash, path);
    if select_open(controller, &key, preview) {
        return;
    }
    let Some(window_root) = controller.project_root() else {
        return;
    };
    let window_root = window_root.canonicalize().unwrap_or(window_root);
    let tab = DiffTab::new(
        controller,
        window_root,
        repo_root.to_path_buf(),
        path.to_path_buf(),
        false,
        Some(Fixed::Commit(hash.clone())),
    );
    let name = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let short: String = hash.as_str().chars().take(7).collect();
    place(controller, tab, key, &tf("{} ({})", &[&name, &short]), preview);
}

/// The key of the tab that shows what the stash entry `hash` changed in `path`.
pub(crate) fn stash_key(hash: &CommitId, path: &Path) -> String {
    format!("diff:stash:{hash}:{}", path.display())
}

/// Shows what the stash entry `hash`, called `name`, changed in `path`, relative to `repo_root`, like [`show`].
pub(crate) fn show_stash(
    controller: &(impl AsWorkspace + ?Sized),
    repo_root: &Path,
    hash: &CommitId,
    name: &str,
    path: &Path,
    preview: bool,
) {
    let controller = &controller.as_workspace();
    let key = stash_key(hash, path);
    if select_open(controller, &key, preview) {
        return;
    }
    let Some(window_root) = controller.project_root() else {
        return;
    };
    let window_root = window_root.canonicalize().unwrap_or(window_root);
    let tab = DiffTab::new(
        controller,
        window_root,
        repo_root.to_path_buf(),
        path.to_path_buf(),
        false,
        Some(Fixed::Stash(hash.clone(), name.to_string())),
    );
    let file = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    place(controller, tab, key, &tf("{} ({})", &[&file, name]), preview);
}

fn place(controller: &Workspace, tab: Retained<DiffTab>, key: String, title: &str, preview: bool) {
    crate::host::state().diff_tabs.insert(tab.ivars().id, Weak::from(&*tab));
    tab.reload();
    let replaced = preview.then(|| take_preview(controller.id())).flatten();
    let added = controller.add_tab(TabKind::File, Some(key.clone()), title, tab.into_super());
    if let Some((old, _)) = replaced {
        controller.close_tab(old);
    }
    if preview && let Some(added) = added {
        PREVIEWS.with(|previews| previews.borrow_mut().insert(controller.id(), (added, key)));
    }
}

thread_local! {
    /// Each workspace's diff tab that the next single click in the panel replaces, with its key.
    static PREVIEWS: RefCell<HashMap<WorkspaceId, (TabId, String)>> = RefCell::new(HashMap::new());
}

/// The preview tab of `workspace`, which is no longer one; `None` once the user closed it.
fn take_preview(workspace: WorkspaceId) -> Option<(TabId, String)> {
    let (tab, key) = PREVIEWS.with(|previews| previews.borrow_mut().remove(&workspace))?;
    let open = Workspace::new(workspace).tab_with_key(&key) == Some(tab);
    open.then_some((tab, key))
}

/// The tab of `key` in `controller`, brought to the front and kept unless `preview`; whether there is one.
fn select_open(controller: &Workspace, key: &str, preview: bool) -> bool {
    if controller.tab_with_key(key).is_none() {
        return false;
    }
    if !preview {
        PREVIEWS.with(|previews| {
            let mut previews = previews.borrow_mut();
            if previews.get(&controller.id()).is_some_and(|(_, shown)| shown == key) {
                previews.remove(&controller.id());
            }
        });
    }
    controller.select_tab(key);
    true
}

impl DiffTab {
    fn new(
        controller: &Workspace,
        window_root: PathBuf,
        repo_root: PathBuf,
        path: PathBuf,
        staged: bool,
        commit: Option<Fixed>,
    ) -> Retained<DiffTab> {
        let mtm: MainThreadMarker = controller.mtm();
        let ivars = DiffIvars {
            id: crate::host::state().diff_tabs.next_id(),
            controller: *controller,
            window_root,
            repo_root,
            path,
            staged,
            commit,
            ui: OnceCell::new(),
            state: RefCell::new(State::default()),
        };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let tab: Retained<DiffTab> =
            unsafe { msg_send![super(DiffTab::alloc(mtm).set_ivars(ivars)), initWithFrame: frame] };
        let ivars = tab.ivars();
        let side = match &ivars.commit {
            Some(Fixed::Commit(hash)) => tf("Commit {}", &[&hash.as_str().chars().take(7).collect::<String>()]),
            Some(Fixed::Stash(_, name)) => tf("Stash {}", &[name]),
            None if ivars.staged => t("Vorgemerkte Änderungen").to_string(),
            None => t("Nicht vorgemerkte Änderungen").to_string(),
        };
        let _ = ivars.ui.set(Ui::new(mtm, &tab, &ivars.path, &side));
        tab
    }

    fn ui(&self) -> &Ui {
        self.ivars().ui.get().expect("the tab builds its views in `new`")
    }

    /// How git may treat the repository, as the workspace's trust says now.
    fn policy(&self) -> Policy {
        crate::refresh::policy_of(&self.ivars().controller)
    }

    fn row(&self, row: isize) -> Option<Row> {
        let state = self.ivars().state.borrow();
        usize::try_from(row).ok().and_then(|r| state.rows.get(r).copied())
    }

    fn line_kind(&self, row: isize) -> Option<LineKind> {
        let Some(Row::Line(hunk, line)) = self.row(row) else {
            return None;
        };
        let state = self.ivars().state.borrow();
        Some(state.diff.as_ref()?.hunks.get(hunk)?.lines.get(line)?.kind)
    }

    /// Reads the diff again, or once more after the read that runs.
    pub(crate) fn reload(&self) {
        {
            let mut state = self.ivars().state.borrow_mut();
            if state.loading {
                state.again = true;
                return;
            }
            state.loading = true;
        }
        let ivars = self.ivars();
        let (root, path, staged, id) = (ivars.repo_root.clone(), ivars.path.clone(), ivars.staged, ivars.id);
        let commit = ivars.commit.clone();
        let policy = self.policy();
        let queued = work::local(
            move || match commit {
                Some(Fixed::Commit(hash)) => load_commit(&root, policy, &path, &hash),
                Some(Fixed::Stash(hash, _)) => load_stash(&root, policy, &path, &hash),
                None => load(&root, policy, &path, staged),
            },
            move |result| with_tab(id, |tab| tab.loaded(result)),
        );
        if !queued {
            self.ivars().state.borrow_mut().loading = false;
        }
    }

    fn loaded(&self, result: Option<Result<Option<FileDiff>, Problem>>) {
        let again = {
            let mut state = self.ivars().state.borrow_mut();
            state.loading = false;
            std::mem::take(&mut state.again)
        };
        match result {
            Some(Ok(diff)) => self.show_diff(diff),
            Some(Err(problem)) => self.ui().show_message(Some(&problem.message)),
            None => self.ui().show_message(Some(t("Git ist unerwartet abgebrochen."))),
        }
        if again {
            self.reload();
        }
    }

    fn show_diff(&self, diff: Option<FileDiff>) {
        {
            let mut state = self.ivars().state.borrow_mut();
            if state.loaded && state.diff == diff {
                return;
            }
            state.loaded = true;
            state.rows = diff.as_ref().map(diff_view::rows).unwrap_or_default();
            state.diff = diff;
        }
        let state = self.ivars().state.borrow();
        let ui = self.ui();
        ui.show_notice(diff_view::notice(state.diff.as_ref(), self.ivars().staged));
        ui.fit_text_column(state.diff.as_ref());
        drop(state);
        ui.table.reloadData();
    }

    fn hunk_action(&self, action: HunkAction, index: Option<usize>) {
        let Some(index) = index else { return };
        if action != HunkAction::Discard {
            self.run_hunk(action, index);
            return;
        }
        let Some(window) = self.window() else { return };
        let id = self.ivars().id;
        sheets::confirm(
            &window,
            t("Abschnitt verwerfen?"),
            t("Die Änderungen dieses Abschnitts gehen verloren. Das lässt sich nicht widerrufen."),
            t("Verwerfen"),
            move || with_tab(id, |tab| tab.run_hunk(HunkAction::Discard, index)),
        );
    }

    /// Stages, unstages or discards the hunk at `index` of the diff on view.
    pub(crate) fn run_hunk(&self, action: HunkAction, index: usize) {
        let diff = {
            let mut state = self.ivars().state.borrow_mut();
            if state.busy {
                return;
            }
            let Some(diff) = state.diff.clone() else { return };
            state.busy = true;
            diff
        };
        let root = self.ivars().repo_root.clone();
        let id = self.ivars().id;
        let policy = self.policy();
        let queued = work::local(
            move || {
                let repo = work::git().ok_or(Error::GitMissing)?.discover(&root, policy)?;
                match action {
                    HunkAction::Stage => repo.stage_hunks(&diff, &[index]),
                    HunkAction::Unstage => repo.unstage_hunks(&diff, &[index]),
                    HunkAction::Discard => repo.discard_hunks(&diff, &[index]),
                }
            },
            move |result| with_tab(id, |tab| tab.hunk_done(result)),
        );
        if !queued {
            self.ivars().state.borrow_mut().busy = false;
        }
    }

    fn hunk_done(&self, result: Option<term_git::Result<()>>) {
        self.ivars().state.borrow_mut().busy = false;
        match result {
            Some(Ok(())) => self.ui().show_message(None),
            Some(Err(error)) => self.ui().show_message(Some(&problem(&error).message)),
            None => self.ui().show_message(Some(t("Git ist unerwartet abgebrochen."))),
        }
        self.reload();
        for panel in super::panel::for_root(&self.ivars().window_root) {
            panel.start_refresh();
        }
    }

    /// Copies the selected lines with their markers.
    fn copy_selection(&self) {
        let ui = self.ui();
        let state = self.ivars().state.borrow();
        let Some(diff) = state.diff.as_ref() else { return };
        let mut text = String::new();
        for (row, entry) in state.rows.iter().enumerate() {
            if !isize::try_from(row).is_ok_and(|r| ui.table.isRowSelected(r)) {
                continue;
            }
            match *entry {
                Row::Hunk(index) => text.push_str(&diff_view::hunk_title(&diff.hunks[index])),
                Row::Line(hunk, line) => {
                    let line = &diff.hunks[hunk].lines[line];
                    text.push_str(diff_view::marker(line.kind));
                    text.push_str(&line.text());
                }
            }
            text.push('\n');
        }
        if text.is_empty() {
            return;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&ns(&text), constant!(NSPasteboardTypeString));
    }
}

/// The hunk a button stands for, from its tag.
fn tag(sender: Option<&AnyObject>) -> Option<usize> {
    sender
        .and_then(|s| s.downcast_ref::<NSControl>())
        .and_then(|c| usize::try_from(c.tag()).ok())
}

/// The diff of `path` on the git lane; `None` when the file has no changes on this side any more.
fn load(root: &Path, policy: Policy, path: &Path, staged: bool) -> Result<Option<FileDiff>, Problem> {
    let read = || -> term_git::Result<Option<FileDiff>> {
        let repo = work::git().ok_or(Error::GitMissing)?.discover(root, policy)?;
        let status = repo.status(StatusOptions::default())?;
        let Some(entry) = status.entries.iter().find(|e| e.path == path) else {
            return Ok(None);
        };
        let side = if staged { entry.index } else { entry.worktree };
        let listed = !staged
            && matches!(
                entry.kind,
                term_git::EntryKind::Untracked | term_git::EntryKind::Unmerged(_)
            );
        if side == FileState::Unmodified && !listed {
            return Ok(None);
        }
        let source = if staged {
            DiffSource::Staged
        } else {
            DiffSource::Unstaged
        };
        repo.diff_entry(entry, source)
    };
    read().map_err(|error| problem(&error))
}

/// What the commit `hash` changed in `path`.
fn load_commit(root: &Path, policy: Policy, path: &Path, hash: &CommitId) -> Result<Option<FileDiff>, Problem> {
    let read = || -> term_git::Result<Option<FileDiff>> {
        let repo = work::git().ok_or(Error::GitMissing)?.discover(root, policy)?;
        repo.diff_commit(hash, path)
    };
    read().map_err(|error| problem(&error))
}

/// What the stash entry `hash` changed in `path`.
fn load_stash(root: &Path, policy: Policy, path: &Path, hash: &CommitId) -> Result<Option<FileDiff>, Problem> {
    let read = || -> term_git::Result<Option<FileDiff>> {
        let repo = work::git().ok_or(Error::GitMissing)?.discover(root, policy)?;
        repo.diff_stash(hash, path)
    };
    read().map_err(|error| problem(&error))
}

/// Whether the unstaged side of `diff` can drop a hunk: a new file goes to the Trash as a whole instead.
pub(crate) fn can_discard(diff: &FileDiff) -> bool {
    diff.change != Change::Added
}

#[cfg(feature = "selftest")]
impl DiffTab {
    /// Rows and hunks on view.
    pub(crate) fn counts(&self) -> (usize, usize) {
        let state = self.ivars().state.borrow();
        (state.rows.len(), state.diff.as_ref().map_or(0, |d| d.hunks.len()))
    }

    /// Selects every row, as ⌘A in the table does.
    pub(crate) fn select_all(&self) {
        let table = &self.ui().table;
        let rows = objc2_foundation::NSRange::new(0, usize::try_from(table.numberOfRows()).unwrap_or(0));
        table.selectRowIndexes_byExtendingSelection(
            &objc2_foundation::NSIndexSet::indexSetWithIndexesInRange(rows),
            false,
        );
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
