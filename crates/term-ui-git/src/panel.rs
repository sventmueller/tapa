//! The Git panel of one workspace as AppKit sees it: the data source and delegate of its list, and the actions of its
//! buttons and menus, which go on to the panel's operations.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSMenu, NSMenuDelegate, NSOutlineView, NSOutlineViewDataSource,
    NSOutlineViewDelegate, NSTableColumn, NSTableRowView, NSTextDelegate, NSTextView, NSTextViewDelegate, NSView,
};
use objc2_foundation::{NSDateFormatter, NSNotification, NSObjectProtocol, NSUndoManager};
use term_git::Repository;

use super::model::{Read, RowKey, State, Task};
use super::ui::Ui;
use term_ui::kit::panel::{KeptCells, RowItem, RowItems};
use term_ui::kit::registry::WorkspacePart;
use term_ui::kit::{guarded, ns};
use term_ui::workspace_host::{AsWorkspace, Trust, Workspace};

pub(crate) struct PanelIvars {
    id: u64,
    controller: Workspace,
    /// The workspace's base path, which may lie below the repository's root.
    root: PathBuf,
    ui: OnceCell<Ui>,
    state: RefCell<State>,
    /// The objects that stand for rows in the outline view, which keeps no reference of its own; the same object
    /// for a key for as long as the row exists, so the view keeps it open and selected.
    items: RowItems<RowKey>,
    /// The cells built for rows, kept while the rows show the same.
    cells: KeptCells<RowKey>,
    /// Writes the times of commits in their tooltips.
    dates: OnceCell<Retained<NSDateFormatter>>,
    /// The keys below each row the outline asked for, kept until the state changes.
    children: RefCell<HashMap<Option<RowKey>, Rc<[RowKey]>>>,
    /// The state changed while the page was out of view, so the list reloads when it comes into view.
    stale: Cell<bool>,
    /// The commit message's own undo, apart from the window's, so its steps stay with the message.
    undo: Retained<NSUndoManager>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `GitPage` does not implement Drop.
    /// The panel's page, which tells the panel of the id it holds when it comes into view.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitPage"]
    #[ivars = u64]
    pub(crate) struct GitPage;

    unsafe impl NSObjectProtocol for GitPage {}

    impl GitPage {
        #[unsafe(method(viewDidUnhide))]
        fn did_unhide(&self) {
            guarded((), || with_panel(*self.ivars(), GitPanel::came_into_view));
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            guarded((), || with_panel(*self.ivars(), GitPanel::came_into_view));
        }
    }
);

impl GitPage {
    /// The page of the panel `panel`.
    pub(crate) fn new(mtm: MainThreadMarker, panel: u64) -> Retained<GitPage> {
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(GitPage::alloc(mtm).set_ivars(panel)), init] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `GitPanel` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitPanel"]
    #[ivars = PanelIvars]
    pub(crate) struct GitPanel;

    unsafe impl NSObjectProtocol for GitPanel {}

    unsafe impl NSOutlineViewDataSource for GitPanel {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn children_count(&self, _view: &NSOutlineView, item: Option<&AnyObject>) -> isize {
            guarded(0, || isize::try_from(self.children(item).len()).unwrap_or(0))
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child(&self, _view: &NSOutlineView, index: isize, item: Option<&AnyObject>) -> Retained<AnyObject> {
            let found = guarded(None, || {
                let children = self.children(item);
                let key = children.get(usize::try_from(index).ok()?)?;
                Some(self.item_for(key))
            });
            found.map_or_else(|| ns("").into(), Into::into)
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn expandable(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            guarded(false, || key_of(item).is_some_and(|key| self.read().expandable(&key)))
        }
    }

    unsafe impl NSControlTextEditingDelegate for GitPanel {}

    unsafe impl NSOutlineViewDelegate for GitPanel {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn view_for(
            &self,
            _view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            guarded(None, || self.kept_row_view(&key_of(item)?))
        }

        #[unsafe(method_id(outlineView:rowViewForItem:))]
        fn row_view_for(&self, view: &NSOutlineView, item: &AnyObject) -> Option<Retained<NSTableRowView>> {
            guarded(None, || match key_of(item)? {
                RowKey::Section(_) => None,
                _ => Some(Retained::into_super(super::graph::GraphRowView::new(view.mtm()))),
            })
        }

        #[unsafe(method(outlineView:heightOfRowByItem:))]
        fn row_height(&self, _view: &NSOutlineView, item: &AnyObject) -> f64 {
            guarded(24.0, || super::rows::height(key_of(item).as_ref()))
        }

        #[unsafe(method(outlineViewItemDidCollapse:))]
        fn did_collapse(&self, notification: &NSNotification) {
            guarded((), || self.note_expansion(notification, false));
        }

        #[unsafe(method(outlineViewItemDidExpand:))]
        fn did_expand(&self, notification: &NSNotification) {
            guarded((), || self.note_expansion(notification, true));
        }
    }

    unsafe impl NSMenuDelegate for GitPanel {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            guarded((), || self.fill_menu(menu));
        }
    }

    unsafe impl NSTextDelegate for GitPanel {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _notification: &NSNotification) {
            guarded((), || self.update_placeholder());
        }
    }

    unsafe impl NSTextViewDelegate for GitPanel {
        #[unsafe(method_id(undoManagerForTextView:))]
        fn undo_manager_for(&self, _view: &NSTextView) -> Option<Retained<NSUndoManager>> {
            Some(self.ivars().undo.clone())
        }
    }

    impl GitPanel {
        #[unsafe(method(termGitStage:))]
        fn stage_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.stage(self.keys_for(sender)));
        }

        #[unsafe(method(termGitUnstage:))]
        fn unstage_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.unstage(self.keys_for(sender)));
        }

        #[unsafe(method(termGitDiscard:))]
        fn discard_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.discard(self.keys_for(sender)));
        }

        #[unsafe(method(termGitOpen:))]
        fn open_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.open(self.keys_for(sender)));
        }

        #[unsafe(method(termGitResolve:))]
        fn resolve_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.resolve(self.keys_for(sender)));
        }

        #[unsafe(method(termGitContinue:))]
        fn continue_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if self.read().stash_apply.is_some() {
                    self.finish_stash_conflict();
                } else {
                    self.continue_operation();
                }
            });
        }

        #[unsafe(method(termGitAbort:))]
        fn abort_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if self.read().stash_apply.is_some() {
                    self.abort_stash_conflict();
                } else {
                    self.abort_operation();
                }
            });
        }

        #[unsafe(method(termGitDiff:))]
        fn diff_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(key) = self.keys_for(sender).first() {
                    self.show_diff(key, false);
                }
            });
        }

        #[unsafe(method(termGitCopyHash:))]
        fn copy_hash_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.run_history_action(objc2::sel!(termGitCopyHash:), &self.keys_for(sender)));
        }

        #[unsafe(method(termGitCopyMessage:))]
        fn copy_message_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.run_history_action(objc2::sel!(termGitCopyMessage:), &self.keys_for(sender)));
        }

        #[unsafe(method(termGitBranchHere:))]
        fn branch_here_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.run_history_action(objc2::sel!(termGitBranchHere:), &self.keys_for(sender)));
        }

        #[unsafe(method(termGitCommitPush:))]
        fn commit_push_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.commit_then(Some(Task::Push)));
        }

        #[unsafe(method(termGitCommitSync:))]
        fn commit_sync_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.commit_then(Some(Task::Pull)));
        }

        #[unsafe(method(termGitStash:))]
        fn stash_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.ask_stash());
        }

        #[unsafe(method(termGitStashStaged:))]
        fn stash_staged_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.stash_staged());
        }

        #[unsafe(method(termGitApplyStash:))]
        fn apply_stash_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.apply_stash(&self.keys_for(sender), term_git::Unstash::Apply));
        }

        #[unsafe(method(termGitPopStash:))]
        fn pop_stash_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.apply_stash(&self.keys_for(sender), term_git::Unstash::Pop));
        }

        #[unsafe(method(termGitDropStash:))]
        fn drop_stash_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.drop_stash(&self.keys_for(sender)));
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &objc2_app_kit::NSMenuItem) -> bool {
            guarded(false, || {
                let keys = self.read().menu_keys.clone();
                item.action().is_none_or(|action| self.can(action, &keys))
            })
        }

        #[unsafe(method(termGitRowClicked:))]
        fn clicked(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.preview_row(self.ui().outline.clickedRow()));
        }

        #[unsafe(method(termGitRowDoubleClicked:))]
        fn double_clicked(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.activate_row(self.ui().outline.clickedRow()));
        }

        #[unsafe(method(termGitStageAll:))]
        fn stage_all_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.stage_all());
        }

        #[unsafe(method(termGitUnstageAll:))]
        fn unstage_all_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.unstage_all());
        }

        #[unsafe(method(termGitCommit:))]
        fn commit_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.commit());
        }

        #[unsafe(method(termGitAmend:))]
        fn amend_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.update_placeholder());
        }

        #[unsafe(method(termGitFetch:))]
        fn fetch_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.remote(Task::Fetch));
        }

        #[unsafe(method(termGitPull:))]
        fn pull_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.remote(Task::Pull));
        }

        #[unsafe(method(termGitPush:))]
        fn push_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.remote(Task::Push));
        }

        #[unsafe(method(termGitCancel:))]
        fn cancel_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(cancel) = self.state().busy.as_ref().and_then(|b| b.cancel.clone()) {
                    cancel.cancel();
                }
            });
        }

        #[unsafe(method(termGitBranch:))]
        fn branch_action(&self, sender: Option<&AnyObject>) {
            guarded((), || self.branch_chosen(sender));
        }

        #[unsafe(method(termGitNewBranch:))]
        fn new_branch_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.new_branch());
        }

        #[unsafe(method(termGitInit:))]
        fn init_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.init_repository());
        }

        #[unsafe(method(termGitRetry:))]
        fn retry_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.start_refresh());
        }

        #[unsafe(method(termGitTrust:))]
        fn trust_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(controller) = self.controller() {
                    controller.ask_trust_again();
                }
            });
        }

        #[unsafe(method(termGitShowOutput:))]
        fn show_output_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.show_problem_output());
        }

        #[unsafe(method(termGitDismissProblem:))]
        fn dismiss_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.set_problem(None));
        }
    }
);

/// Runs `f` with the panel `id`, if its workspace is still open.
pub(crate) fn with_panel(id: u64, f: impl FnOnce(&GitPanel)) {
    crate::host::state().panels.with(id, f);
}

/// The panels of workspaces whose base path is `root`.
pub(crate) fn for_root(root: &Path) -> Vec<Retained<GitPanel>> {
    crate::host::state().panels.for_root(root)
}

/// The panel of `controller`'s workspace.
pub(crate) fn of(controller: &(impl AsWorkspace + ?Sized)) -> Option<Retained<GitPanel>> {
    let controller = &controller.as_workspace();
    crate::host::state().panels.of(*controller)
}

impl WorkspacePart for GitPanel {
    fn workspace(&self) -> Option<Workspace> {
        self.controller()
    }

    fn root(&self) -> &Path {
        GitPanel::root(self)
    }
}

impl GitPanel {
    pub(super) fn new(mtm: MainThreadMarker, controller: &Workspace, root: PathBuf) -> Retained<GitPanel> {
        let ivars = PanelIvars {
            id: crate::host::state().panels.next_id(),
            controller: *controller,
            root,
            ui: OnceCell::new(),
            state: RefCell::new(State::default()),
            items: RowItems::default(),
            cells: KeptCells::default(),
            dates: OnceCell::new(),
            children: RefCell::new(HashMap::new()),
            stale: Cell::new(false),
            undo: NSUndoManager::new(mtm),
        };
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        let panel: Retained<GitPanel> = unsafe { msg_send![super(GitPanel::alloc(mtm).set_ivars(ivars)), init] };
        let _ = panel.ivars().ui.set(Ui::new(mtm, &panel));
        panel
    }

    pub(crate) fn id(&self) -> u64 {
        self.ivars().id
    }

    pub(crate) fn root(&self) -> &Path {
        &self.ivars().root
    }

    pub(crate) fn ui(&self) -> &Ui {
        self.ivars().ui.get().expect("the panel builds its views in `new`")
    }

    pub(crate) fn state(&self) -> std::cell::RefMut<'_, State> {
        let state = self.ivars().state.borrow_mut();
        self.ivars().children.borrow_mut().clear();
        state
    }

    /// Whether the page is out of view, where the list waits with reloading until it comes back.
    pub(super) fn out_of_view(&self) -> bool {
        let page = &self.ui().page;
        page.window().is_none() || page.isHiddenOrHasHiddenAncestor()
    }

    /// Notes whether the list waits to reload until the page comes into view.
    pub(super) fn set_stale(&self, stale: bool) {
        self.ivars().stale.set(stale);
    }

    /// The page came into view: the list catches up with what changed while it was away.
    fn came_into_view(&self) {
        if self.ivars().stale.get() && !self.out_of_view() {
            self.reload_outline();
        }
    }

    pub(crate) fn read(&self) -> std::cell::Ref<'_, State> {
        self.ivars().state.borrow()
    }

    pub(crate) fn controller(&self) -> Option<Workspace> {
        Workspace::open(self.ivars().controller.id())
    }

    /// How many commits the graph reads.
    pub(crate) fn history_limit(&self) -> usize {
        self.read().history_limit()
    }

    pub(crate) fn repo(&self) -> Option<Repository> {
        self.ivars().state.borrow().repo.clone()
    }

    /// The rows below `item`, the sections for the top.
    pub(crate) fn children(&self, item: Option<&AnyObject>) -> Rc<[RowKey]> {
        let parent = match item {
            None => None,
            Some(item) => match key_of(item) {
                Some(key) => Some(key),
                None => return Rc::from([]),
            },
        };
        if let Some(found) = self.ivars().children.borrow().get(&parent) {
            return found.clone();
        }
        let found: Rc<[RowKey]> = self.read().children(parent.as_ref()).into();
        self.ivars().children.borrow_mut().insert(parent, found.clone());
        found
    }

    /// The object that stands for `key`, the same one for as long as its row exists.
    pub(crate) fn item_for(&self, key: &RowKey) -> Retained<RowItem> {
        self.ivars().items.item(self.mtm(), key)
    }

    /// Lets go of the items and cells of rows the panel no longer has, after the outline view read the rows anew.
    pub(super) fn forget_gone_items(&self) {
        let live = self.read().all_keys();
        self.ivars().items.keep(|key| live.contains(key));
        self.ivars().cells.keep(|key| live.contains(key));
    }

    /// The cells built for rows.
    pub(crate) fn cells(&self) -> &KeptCells<RowKey> {
        &self.ivars().cells
    }

    /// The formatter of the day and time in the user's format.
    pub(crate) fn dates(&self) -> &NSDateFormatter {
        self.ivars().dates.get_or_init(|| {
            let formatter = NSDateFormatter::new();
            formatter.setDateStyle(objc2_foundation::NSDateFormatterStyle::LongStyle);
            formatter.setTimeStyle(objc2_foundation::NSDateFormatterStyle::ShortStyle);
            formatter
        })
    }

    fn note_expansion(&self, notification: &NSNotification, expanded: bool) {
        let Some(info) = notification.userInfo() else { return };
        let Some(key) = info.objectForKey(&ns("NSObject")).and_then(|item| key_of(&item)) else {
            return;
        };
        let read = self.state().expansion(key, expanded);
        match read {
            Some(Read::CommitChanges(hash)) => self.read_changes(&hash),
            Some(Read::StashChanges(hash)) => self.read_stash_changes(&hash),
            None => {}
        }
    }
}

/// The key `item` stands for.
pub(crate) fn key_of(item: &AnyObject) -> Option<RowKey> {
    RowItems::<RowKey>::key_of(item)
}
