//! The sidebar on screen: a source list of the window's workspaces, one row per `Workspace`, which tells the app what
//! the user picked; or, in its place, a panel of the selected workspace with the way back to the list (E22).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSControl, NSControlTextEditingDelegate, NSDragOperation, NSDraggingInfo, NSMenu,
    NSMenuDelegate, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSPasteboardWriting, NSScrollView, NSTableColumn,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewDropOperation, NSTableViewRowSizeStyle,
    NSTableViewStyle, NSTextFieldDelegate, NSTextView, NSView, NSWorkspace,
};
use objc2_foundation::{NSArray, NSIndexSet, NSNotification, NSObject, NSObjectProtocol, NSURL};
use splat_i18n::t;
use term_ui::kit::{self, guarded, ns};

use crate::cell::{Cell as RowCell, Size};
use crate::model::{Panel, Workspace};

/// What the sidebar calls with the id of the workspace the user picked.
type Picked = Box<dyn Fn(u64)>;

/// What the sidebar calls with a workspace's id and what the user chose for it in the row's context menu.
type Acted = Box<dyn Fn(u64, RowAction)>;

/// What the sidebar calls with a workspace's id and the panel the user opened in its row or closed by going back.
type Toggled = Box<dyn Fn(u64, Panel)>;

/// What the sidebar calls with a workspace's id and the name the user gave it in its row.
pub(crate) type Renamed = Box<dyn Fn(u64, String)>;

/// A panel that shows in place of the list: whose and which it is, its title, its view and the buttons of its title
/// row.
pub struct PanelView {
    /// The workspace's id.
    pub workspace: u64,
    /// Which panel.
    pub panel: Panel,
    /// Its title.
    pub title: String,
    /// What it shows; none in the sidebars of the tab windows the user does not work in, as a view lives in one
    /// window only.
    pub content: Option<Retained<NSView>>,
    /// The buttons at the end of its title row.
    pub accessory: Option<Retained<NSView>>,
}

/// What a row's context menu asks of the app for its workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    /// A new tab in the workspace, in its base folder.
    NewTab,
    /// Give the workspace another name.
    Rename,
    /// Close the workspace with its tabs.
    Close,
    /// Trust the workspace, or take the trust back (E21).
    ToggleTrust,
}

/// What the sidebar keeps.
pub struct Ivars {
    pub(crate) entries: RefCell<Vec<Workspace>>,
    selected: Cell<Option<u64>>,
    home: PathBuf,
    table: RefCell<Option<Retained<NSTableView>>>,
    picked: RefCell<Option<Picked>>,
    acted: RefCell<Option<Acted>>,
    /// The list of workspaces, the panel's place, and whose panel shows there in place of the list.
    list: RefCell<Option<Retained<NSView>>>,
    chrome: RefCell<Option<crate::panel_view::Chrome>>,
    panel: Cell<Option<(u64, Panel)>>,
    toggled: RefCell<Option<Toggled>>,
    /// What the settings button at the foot opens.
    settings: RefCell<Option<Box<dyn Fn()>>>,
    gear: RefCell<Option<Retained<NSButton>>>,
    /// The digits of the largest number of changed files the rows were made for.
    count_digits: Cell<usize>,
    /// Set while the app selects a row, so that the selection does not come back to it as the user's pick.
    quiet: Cell<bool>,
    pub(crate) renamed: RefCell<Option<Renamed>>,
    /// The workspace whose name the user edits in its row, with the name it had.
    pub(crate) editing: RefCell<Option<crate::rename::Editing>>,
    pub(crate) moved: RefCell<Option<crate::reorder::Moved>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Sidebar` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSidebar"]
    #[ivars = Ivars]
    /// The list of a window's workspaces beside the tabs of the one selected.
    pub struct Sidebar;

    unsafe impl NSObjectProtocol for Sidebar {}

    unsafe impl NSTextFieldDelegate for Sidebar {}

    unsafe impl NSControlTextEditingDelegate for Sidebar {
        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_text_did_end_editing(&self, _notification: &NSNotification) {
            guarded((), || self.end_rename());
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            guarded(false, || self.rename_command(command))
        }
    }

    unsafe impl NSTableViewDataSource for Sidebar {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn row_count(&self, _table: &NSTableView) -> isize {
            guarded(0, || isize::try_from(self.ivars().entries.borrow().len()).unwrap_or(0))
        }

        #[unsafe(method_id(tableView:pasteboardWriterForRow:))]
        fn pasteboard_writer(
            &self,
            _table: &NSTableView,
            row: isize,
        ) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
            guarded(None, || self.drag_writer(row))
        }

        #[unsafe(method(tableView:validateDrop:proposedRow:proposedDropOperation:))]
        fn validate_drop_at(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            operation: NSTableViewDropOperation,
        ) -> NSDragOperation {
            guarded(NSDragOperation::None, || self.validate_drop(table, info, row, operation))
        }

        #[unsafe(method(tableView:acceptDrop:row:dropOperation:))]
        fn accept_drop_at(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            _operation: NSTableViewDropOperation,
        ) -> bool {
            guarded(false, || self.accept_drop(table, info, row))
        }
    }

    unsafe impl NSTableViewDelegate for Sidebar {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            guarded(None, || {
                let entries = self.ivars().entries.borrow();
                let entry = entries.get(usize::try_from(row).ok()?)?;
                let toggles = self.ivars().toggled.borrow().is_some();
                let size = Size::of(self.ivars().count_digits.get());
                let target: &AnyObject = self;
                let cell = RowCell::new(self.mtm(), entry, &self.ivars().home, &size, toggles.then_some(target));
                Some(Retained::into_super(Retained::into_super(cell)))
            })
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of_row(&self, _table: &NSTableView, _row: isize) -> f64 {
            guarded(28.0, || Size::of(0).row_height())
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select(&self, _table: &NSTableView, row: isize) -> bool {
            guarded(false, || self.workspace_at(row).is_some())
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _notification: &NSNotification) {
            guarded((), || self.user_picked());
        }
    }

    unsafe impl NSMenuDelegate for Sidebar {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            guarded((), || self.fill_menu(menu));
        }
    }

    impl Sidebar {
        #[unsafe(method(termSidebarNewTab:))]
        fn new_tab(&self, item: &NSMenuItem) {
            guarded((), || self.act(item, RowAction::NewTab));
        }

        #[unsafe(method(termSidebarRename:))]
        fn rename(&self, item: &NSMenuItem) {
            guarded((), || {
                if let Some(entry) = self.entry_of(item) {
                    self.begin_rename(entry.id);
                }
            });
        }

        #[unsafe(method(termSidebarEditName:))]
        fn edit_name(&self, sender: &NSButton) {
            guarded((), || {
                if let Ok(id) = u64::try_from(sender.tag()) {
                    self.begin_rename(id);
                }
            });
        }

        #[unsafe(method(termSidebarToggleFiles:))]
        fn toggle_files(&self, sender: &NSButton) {
            guarded((), || self.toggle(sender.tag(), Panel::Files));
        }

        #[unsafe(method(termSidebarToggleGit:))]
        fn toggle_git(&self, sender: &NSButton) {
            guarded((), || self.toggle(sender.tag(), Panel::Git));
        }

        #[unsafe(method(termSidebarSettings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(settings) = self.ivars().settings.borrow().as_ref() {
                    settings();
                }
            });
        }

        #[unsafe(method(termSidebarBack:))]
        fn back(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.go_back());
        }

        #[unsafe(method(termSidebarToggleTrust:))]
        fn toggle_trust(&self, item: &NSMenuItem) {
            guarded((), || self.act(item, RowAction::ToggleTrust));
        }

        #[unsafe(method(termSidebarClose:))]
        fn close(&self, item: &NSMenuItem) {
            guarded((), || self.act(item, RowAction::Close));
        }

        #[unsafe(method(termSidebarShowInFinder:))]
        fn show_in_finder(&self, item: &NSMenuItem) {
            guarded((), || {
                if let Some(entry) = self.entry_of(item) {
                    let url = NSURL::fileURLWithPath(&ns(&entry.base.to_string_lossy()));
                    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
                }
            });
        }

        #[unsafe(method(termSidebarCopyPath:))]
        fn copy_path(&self, item: &NSMenuItem) {
            guarded((), || {
                if let Some(entry) = self.entry_of(item) {
                    let pasteboard = NSPasteboard::generalPasteboard();
                    pasteboard.clearContents();
                    // SAFETY: AppKit sets its exported pasteboard type constants when it loads and never changes them.
                    let kind = unsafe { NSPasteboardTypeString };
                    pasteboard.setString_forType(&ns(&entry.base.to_string_lossy()), kind);
                }
            });
        }
    }
);

impl Sidebar {
    /// A sidebar that calls `picked` with a workspace's id when the user selects its row; and the view to show it in.
    pub fn new(mtm: MainThreadMarker, picked: impl Fn(u64) + 'static) -> (Retained<Sidebar>, Retained<NSView>) {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            entries: RefCell::new(Vec::new()),
            selected: Cell::new(None),
            home: term_base::home(),
            table: RefCell::new(None),
            picked: RefCell::new(Some(Box::new(picked))),
            acted: RefCell::new(None),
            list: RefCell::new(None),
            chrome: RefCell::new(None),
            panel: Cell::new(None),
            toggled: RefCell::new(None),
            settings: RefCell::new(None),
            gear: RefCell::new(None),
            count_digits: Cell::new(0),
            quiet: Cell::new(false),
            renamed: RefCell::new(None),
            editing: RefCell::new(None),
            moved: RefCell::new(None),
        });
        // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
        let this: Retained<Sidebar> = unsafe { msg_send![super(this), init] };
        let table = crate::table::Table::make(mtm, &this);
        table.setStyle(NSTableViewStyle::SourceList);
        table.setHeaderView(None);
        // A source list of the default size style would give the names the system's sidebar size over the interface's.
        table.setRowSizeStyle(NSTableViewRowSizeStyle::Custom);
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns("workspace"));
        column.setResizingMask(objc2_app_kit::NSTableColumnResizingOptions::AutoresizingMask);
        table.addTableColumn(&column);
        table.setColumnAutoresizingStyle(
            objc2_app_kit::NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle,
        );
        // SAFETY: The table holds its data source and delegate weakly; the window keeps the sidebar as long as the table.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(&*this)));
            table.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        }
        table.setAccessibilityLabel(Some(&ns(t("Arbeitsbereiche"))));
        let menu = NSMenu::new(mtm);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        // SAFETY: A plain menu that the table retains; its delegate is the sidebar, which outlives the table's use of it.
        unsafe { table.setMenu(Some(&menu)) };
        term_ui::kit::ui_size::follow_row_heights(&table);
        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        *this.ivars().table.borrow_mut() = Some(table);
        let target: &AnyObject = &this;
        let chrome = crate::panel_view::Chrome::new(mtm, target, sel!(termSidebarBack:));
        let container = NSView::new(mtm);
        let gear = crate::footer::place(
            mtm,
            &container,
            &[&*scroll as &NSView, &chrome.view],
            target,
            sel!(termSidebarSettings:),
        );
        *this.ivars().gear.borrow_mut() = Some(gear);
        chrome.view.setHidden(true);
        *this.ivars().list.borrow_mut() = Some(Retained::into_super(scroll));
        *this.ivars().chrome.borrow_mut() = Some(chrome);
        (this, container)
    }

    /// Shows `entries` with the workspace `selected` picked.
    pub fn show(&self, entries: Vec<Workspace>, selected: Option<u64>) {
        let (same_ids, changed) = {
            let old = self.ivars().entries.borrow();
            let same_ids = old.len() == entries.len() && old.iter().zip(&entries).all(|(a, b)| a.id == b.id);
            let changed: Vec<usize> = (0..entries.len().min(old.len()))
                .filter(|at| old[*at] != entries[*at])
                .collect();
            (same_ids, changed)
        };
        let digits = entries
            .iter()
            .map(|entry| entry.changes)
            .max()
            .filter(|most| *most > 0)
            .map_or(0, |most| most.to_string().len());
        let same_digits = self.ivars().count_digits.replace(digits) == digits;
        *self.ivars().entries.borrow_mut() = entries;
        let Some(table) = self.ivars().table.borrow().clone() else {
            return;
        };
        table.sizeLastColumnToFit();
        if !same_ids || !same_digits {
            table.reloadData();
        } else if !changed.is_empty() {
            // Only the rows that changed, so that a row's switch keeps the pointer's hover; never the one whose name
            // the user edits.
            let editing = self.editing_row();
            let indexes = objc2_foundation::NSMutableIndexSet::new();
            for at in changed.into_iter().filter(|at| Some(*at) != editing) {
                indexes.addIndex(at);
            }
            table.reloadDataForRowIndexes_columnIndexes(&indexes, &NSIndexSet::indexSetWithIndex(0));
        }
        self.ivars().selected.set(selected);
        let row = selected.and_then(|id| self.row_of(id));
        if let Some(row) = row {
            let _quiet = kit::FlagGuard::raise(&self.ivars().quiet);
            table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
            table.scrollRowToVisible(isize::try_from(row).unwrap_or(0));
        }
    }

    /// The workspace whose row is selected.
    pub fn selected(&self) -> Option<u64> {
        self.ivars().selected.get()
    }

    fn user_picked(&self) {
        if self.ivars().quiet.get() {
            return;
        }
        let Some(table) = self.ivars().table.borrow().clone() else {
            return;
        };
        let Some(row) = self.workspace_at(table.selectedRow()) else {
            return;
        };
        let Some(id) = self.ivars().entries.borrow().get(row).map(|entry| entry.id) else {
            return;
        };
        if self.ivars().selected.replace(Some(id)) == Some(id) {
            return;
        }
        if let Some(picked) = self.ivars().picked.borrow().as_ref() {
            picked(id);
        }
    }

    /// Has the rows' context menus offer a new tab, renaming and closing, which `acted` does for the workspace with the
    /// given id.
    pub fn on_action(&self, acted: impl Fn(u64, RowAction) + 'static) {
        *self.ivars().acted.borrow_mut() = Some(Box::new(acted));
    }

    /// The context menu of the row the user clicked: a new tab and renaming, its folder, and closing it last as the
    /// step that cannot be undone.
    fn fill_menu(&self, menu: &NSMenu) {
        menu.removeAllItems();
        let Some(table) = self.ivars().table.borrow().clone() else {
            return;
        };
        let Some((id, trusted, fixed)) = self.workspace_at(table.clickedRow()).and_then(|row| {
            self.ivars()
                .entries
                .borrow()
                .get(row)
                .map(|entry| (entry.id, entry.trusted, entry.fixed))
        }) else {
            return;
        };
        let mtm = self.mtm();
        let add = |title: &str, action: Sel| {
            let item = kit::item(mtm, title, Some(action), "");
            item.setTag(isize::try_from(id).unwrap_or(isize::MAX));
            kit::set_target(&item, self as &AnyObject);
            menu.addItem(&item);
        };
        let acts = self.ivars().acted.borrow().is_some();
        if acts {
            add(t("Neuer Tab"), sel!(termSidebarNewTab:));
            if !fixed {
                add(t("Arbeitsbereich umbenennen"), sel!(termSidebarRename:));
            }
            let trust = if trusted {
                t("Vertrauen entziehen")
            } else {
                t("Arbeitsbereich vertrauen")
            };
            add(trust, sel!(termSidebarToggleTrust:));
            menu.addItem(&kit::separator(mtm));
        }
        add(t("Im Finder anzeigen"), sel!(termSidebarShowInFinder:));
        add(t("Pfad kopieren"), sel!(termSidebarCopyPath:));
        if acts && !fixed {
            menu.addItem(&kit::separator(mtm));
            add(t("Arbeitsbereich entfernen"), sel!(termSidebarClose:));
        }
    }

    fn entry_of(&self, item: &NSMenuItem) -> Option<Workspace> {
        let id = u64::try_from(item.tag()).ok()?;
        self.ivars()
            .entries
            .borrow()
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
    }

    /// The place among the entries of the workspace whose row is table row `row`.
    fn workspace_at(&self, row: isize) -> Option<usize> {
        let row = usize::try_from(row).ok()?;
        (row < self.ivars().entries.borrow().len()).then_some(row)
    }

    /// The table row of workspace `id`.
    /// The table of workspaces.
    pub(crate) fn table(&self) -> Option<Retained<NSTableView>> {
        self.ivars().table.borrow().clone()
    }

    /// The workspaces it shows.
    pub(crate) fn entries(&self) -> Vec<Workspace> {
        self.ivars().entries.borrow().clone()
    }

    pub(crate) fn row_of(&self, id: u64) -> Option<usize> {
        self.ivars().entries.borrow().iter().position(|entry| entry.id == id)
    }

    /// Has the settings button at the sidebar's foot run `open`.
    pub fn on_settings(&self, open: impl Fn() + 'static) {
        *self.ivars().settings.borrow_mut() = Some(Box::new(open));
    }

    /// Clicks the settings button at the sidebar's foot, for the self-test; its tooltip, if it shows.
    pub fn press_settings(&self) -> Option<String> {
        let gear = self.ivars().gear.borrow().clone()?;
        // SAFETY: A click without a sender, which sends the button's action to its target as the mouse would.
        unsafe { gear.performClick(None) };
        (!gear.isHiddenOrHasHiddenAncestor()).then(|| gear.toolTip().map(|tip| tip.to_string()).unwrap_or_default())
    }

    /// Shows the two panel switches in each row, which tell `toggled` the workspace and the panel the user opened or
    /// closed; without this the rows have none, as until the panels exist.
    pub fn on_panel_toggle(&self, toggled: impl Fn(u64, Panel) + 'static) {
        *self.ivars().toggled.borrow_mut() = Some(Box::new(toggled));
        if let Some(table) = self.ivars().table.borrow().as_ref() {
            table.reloadData();
        }
    }

    /// Shows `panel` in place of the list, or the list again without one.
    pub fn set_panel(&self, panel: Option<PanelView>) {
        let chrome = self.ivars().chrome.borrow();
        let (Some(chrome), Some(list)) = (chrome.as_ref(), self.ivars().list.borrow().clone()) else {
            return;
        };
        match panel {
            Some(panel) => {
                chrome.show(&panel.title, panel.content.as_deref(), panel.accessory.as_deref());
                self.ivars().panel.set(Some((panel.workspace, panel.panel)));
            }
            None => {
                chrome.clear();
                self.ivars().panel.set(None);
            }
        }
        let showing = self.ivars().panel.get().is_some();
        if chrome.view.isHidden() == showing {
            chrome.view.setHidden(!showing);
            list.setHidden(showing);
        }
    }

    /// Whose panel shows in place of the list and its title, for the self-test.
    pub fn shown_panel(&self) -> Option<(u64, Panel, String)> {
        let (id, panel) = self.ivars().panel.get()?;
        let title = self.ivars().chrome.borrow().as_ref()?.title();
        Some((id, panel, title))
    }

    /// Goes back from the panel to the list, as its button does.
    pub fn go_back(&self) {
        if let Some((id, panel)) = self.ivars().panel.get()
            && let Some(toggled) = self.ivars().toggled.borrow().as_ref()
        {
            toggled(id, panel);
        }
    }

    fn toggle(&self, tag: isize, panel: Panel) {
        let Ok(id) = u64::try_from(tag) else { return };
        if let Some(toggled) = self.ivars().toggled.borrow().as_ref() {
            toggled(id, panel);
        }
    }

    fn act(&self, item: &NSMenuItem, action: RowAction) {
        let Some(id) = self.entry_of(item).map(|entry| entry.id) else {
            return;
        };
        if let Some(acted) = self.ivars().acted.borrow().as_ref() {
            acted(id, action);
        }
    }
}
