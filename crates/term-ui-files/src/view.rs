//! The view of the files panel: the outline view's data source and delegate, which draws the rows and passes clicks,
//! folds and menu commands on to the controller. It is the list's object in the registry, holding its page.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSControlTextEditingDelegate, NSImageAlignment, NSImageView, NSLineBreakMode, NSMenu,
    NSMenuDelegate, NSOutlineView, NSOutlineViewDataSource, NSOutlineViewDelegate, NSProgressIndicator,
    NSProgressIndicatorStyle, NSTableCellView, NSTableColumn, NSTextField, NSUserInterfaceItemIdentification,
    NSUserInterfaceLayoutDirection, NSView,
};
use objc2_foundation::{NSNotification, NSObjectProtocol};
use term_project::Entry;
use term_project::files_panel::{Command, Kind, MenuEntry, Model, Row};
use term_ui::kit::panel::{RowItem, RowItems};
use term_ui::kit::registry::WorkspacePart;
use term_ui::kit::ui_size;
use term_ui::workspace_host::{Workspace, WorkspaceId};

use crate::project::Project;
use term_ui::kit::{activate, auto_layout, guarded, item, ns, separator, set_target};

pub(crate) struct TreeIvars {
    /// The list's number, which readings from other threads carry.
    pub(crate) id: u64,
    /// The workspace whose sidebar entry shows the list.
    pub(crate) workspace: WorkspaceId,
    pub(crate) project: Project,
    /// The outline view the source feeds, which holds the source only weakly.
    pub(crate) outline: RefCell<Weak<NSOutlineView>>,
    /// The page with the tree, which the sidebar shows; it keeps the outline view.
    pub(crate) page: RefCell<Option<Retained<NSView>>>,
    /// The buttons of the panel's title row.
    pub(crate) buttons: RefCell<Option<Retained<NSView>>>,
    /// The pointer's reads ahead over the tree.
    pub(crate) hover: RefCell<Option<Retained<crate::hover::TreeHover>>>,
    pub(crate) model: RefCell<Model>,
    /// The objects that stand for rows in the outline view, one per path while the view may hold its row, since the
    /// view keeps no reference of its own.
    pub(crate) items: RowItems<PathBuf>,
    /// The entries of each open folder as the outline view was last told them.
    pub(crate) shown: RefCell<HashMap<PathBuf, Arc<[Entry]>>>,
    /// The folders opened whose entries are still being read.
    pub(crate) pending: RefCell<HashMap<PathBuf, Pending>>,
    /// Set while the tree opens the folders inside one the user opened with all its folders.
    pub(crate) cascade: Cell<bool>,
    /// Folders that closed whose rows the outline view still holds until it reads them anew.
    pub(crate) closing: RefCell<Vec<PathBuf>>,
    /// The folders rows inside of which may have left the outline view since the items of rows it no longer holds
    /// were let go of.
    pub(crate) stale: RefCell<Vec<PathBuf>>,
}

/// A folder that opened before its entries were read.
pub(crate) struct Pending {
    /// The row that shows a spinner in the folder once reading takes long enough to be seen.
    pub(crate) spinner: Option<Retained<NSObject>>,
    /// The folders inside open as well, as Option asks.
    pub(crate) deep: bool,
}

/// What a folder's rows are as the outline view asks for them.
enum Listed {
    Entries(Arc<[Entry]>),
    Loading(Retained<NSObject>),
    Nothing,
}

/// The identifier of the rows the outline view keeps for reuse.
const ROW: &str = "file";

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `FileTreeSource` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermFilesFileTreeSource"]
    #[ivars = TreeIvars]
    pub(crate) struct FileTreeSource;

    unsafe impl NSObjectProtocol for FileTreeSource {}

    unsafe impl NSOutlineViewDataSource for FileTreeSource {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        fn children_count(&self, _view: &NSOutlineView, item: Option<&AnyObject>) -> isize {
            guarded(0, || match self.children_of(item) {
                Listed::Entries(entries) => isize::try_from(entries.len()).unwrap_or(0),
                Listed::Loading(_) => 1,
                Listed::Nothing => 0,
            })
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        fn child(&self, _view: &NSOutlineView, index: isize, item: Option<&AnyObject>) -> Retained<AnyObject> {
            let found: Option<Retained<AnyObject>> = guarded(None, || match self.children_of(item) {
                Listed::Entries(entries) => Some(self.item_for(entries.get(usize::try_from(index).ok()?)?).into()),
                Listed::Loading(spinner) => Some(spinner.into()),
                Listed::Nothing => None,
            });
            found.map_or_else(|| ns("").into(), Into::into)
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        fn expandable(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            guarded(None, || {
                let path = path_of(item)?;
                self.ivars().model.borrow().row(&path).map(|row| row.expandable)
            })
            .unwrap_or(false)
        }
    }

    unsafe impl NSControlTextEditingDelegate for FileTreeSource {}

    unsafe impl NSOutlineViewDelegate for FileTreeSource {
        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        fn view_for(
            &self,
            view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            guarded(None, || {
                let Some(path) = path_of(item) else {
                    return Some(spinner_view(self.mtm()));
                };
                let row = self.ivars().model.borrow().row(&path).unwrap_or_default();
                // SAFETY: The item came from this outline view, which knows it.
                let open = row.expandable.then(|| unsafe { view.isItemExpanded(Some(item)) });
                // SAFETY: The identifier names cells `row_view` made; no owner is involved.
                let reused = unsafe { view.makeViewWithIdentifier_owner(&ns(ROW), None) };
                let cell = reused.and_then(|cell| cell.downcast::<NSTableCellView>().ok());
                let cell = cell.unwrap_or_else(|| row_view(self.mtm()));
                fill_row(&cell, &path, row.ignored, open);
                Some(cell.into_super())
            })
        }

        #[unsafe(method(outlineViewItemWillExpand:))]
        fn will_expand(&self, notification: &NSNotification) {
            guarded((), || {
                if let Some(path) = expanded_path(notification) {
                    let deep = self.ivars().cascade.get() || option_held(self.mtm());
                    self.folder_opens(&path, deep);
                }
            });
        }

        #[unsafe(method(outlineViewItemDidExpand:))]
        fn did_expand(&self, notification: &NSNotification) {
            guarded((), || turn_chevron(notification, true));
        }

        #[unsafe(method(outlineViewItemDidCollapse:))]
        fn did_collapse(&self, notification: &NSNotification) {
            guarded((), || {
                if let Some(path) = expanded_path(notification) {
                    self.folder_closed(&path);
                }
                turn_chevron(notification, false);
            });
        }
    }

    unsafe impl NSMenuDelegate for FileTreeSource {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            guarded((), || self.fill_menu(menu));
        }
    }

    impl FileTreeSource {
        #[unsafe(method(termRowClicked:))]
        fn row_clicked_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let Some(view) = sender.and_then(|s| s.downcast_ref::<NSOutlineView>()) else { return };
                if !extends_selection(self.mtm()) {
                    self.row_clicked(view, view.clickedRow());
                }
            });
        }

        #[unsafe(method(termRowDoubleClicked:))]
        fn row_double_clicked_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let Some(view) = sender.and_then(|s| s.downcast_ref::<NSOutlineView>()) else { return };
                self.row_double_clicked(view, view.clickedRow());
            });
        }




        #[unsafe(method(termOpenAsPreview:))]
        fn open_as_preview(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::OpenAsPreview));
        }

        #[unsafe(method(termOpenInBrowserTab:))]
        fn open_in_browser_tab(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::OpenInBrowserTab));
        }

        #[unsafe(method(termRevealInFinder:))]
        fn reveal(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::Reveal));
        }

        #[unsafe(method(termCopyPath:))]
        fn copy_path(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::CopyPath));
        }

        #[unsafe(method(termCopyRelativePath:))]
        fn copy_relative_path(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::CopyRelativePath));
        }

        #[unsafe(method(termMoveToTrash:))]
        fn move_to_trash(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::Trash));
        }

        #[unsafe(method(termNewFile:))]
        fn new_file(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::NewFile));
        }

        #[unsafe(method(termNewFolder:))]
        fn new_folder(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::NewFolder));
        }

        #[unsafe(method(termRename:))]
        fn rename(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.command(Command::Rename));
        }

        #[unsafe(method(termTreeNewFile:))]
        fn header_new_file(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.create_from_header(Kind::File));
        }

        #[unsafe(method(termTreeNewFolder:))]
        fn header_new_folder(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.create_from_header(Kind::Folder));
        }

        #[unsafe(method(termTreeCollapseAll:))]
        fn collapse_all(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(view) = self.outline() {
                    // SAFETY: Nil stands for the root, whose items and their children all fold.
                    unsafe { view.collapseItem_collapseChildren(None, true) };
                }
            });
        }
    }
);

impl FileTreeSource {
    pub(crate) fn new(mtm: MainThreadMarker, id: u64, workspace: WorkspaceId, project: Project) -> Retained<Self> {
        let ivars = TreeIvars {
            model: RefCell::new(Model::new(project.root().to_path_buf())),
            id,
            workspace,
            project,
            outline: RefCell::new(Weak::default()),
            page: RefCell::new(None),
            buttons: RefCell::new(None),
            hover: RefCell::new(None),
            items: RowItems::default(),
            shown: RefCell::new(HashMap::new()),
            pending: RefCell::new(HashMap::new()),
            cascade: Cell::new(false),
            closing: RefCell::new(Vec::new()),
            stale: RefCell::new(Vec::new()),
        };
        // SAFETY: NSObject's initializer, called once on the newly allocated object.
        unsafe { msg_send![super(FileTreeSource::alloc(mtm).set_ivars(ivars)), init] }
    }

    /// The outline view the source feeds, while it lives.
    pub(crate) fn outline(&self) -> Option<Retained<NSOutlineView>> {
        self.ivars().outline.borrow().load()
    }

    /// The object that stands for `path` in the outline view, if the view was handed one.
    pub(crate) fn item_of(&self, path: &Path) -> Option<Retained<RowItem>> {
        self.ivars().items.get(&path.to_path_buf())
    }

    fn children_of(&self, item: Option<&AnyObject>) -> Listed {
        let dir = match item {
            Some(item) => match path_of(item) {
                Some(path) => path,
                None => return Listed::Nothing,
            },
            None => self.ivars().project.root().to_path_buf(),
        };
        if let Some(entries) = self.ivars().shown.borrow().get(&dir) {
            return Listed::Entries(entries.clone());
        }
        match self.ivars().pending.borrow().get(&dir).and_then(|p| p.spinner.clone()) {
            Some(spinner) => Listed::Loading(spinner),
            None => Listed::Nothing,
        }
    }

    pub(crate) fn item_for(&self, entry: &Entry) -> Retained<RowItem> {
        self.ivars().model.borrow_mut().note(&entry.path, Row::of(entry));
        self.ivars().items.item(self.mtm(), &entry.path)
    }

    fn fill_menu(&self, menu: &NSMenu) {
        menu.removeAllItems();
        let mtm = self.mtm();
        let Some(view) = self.outline() else {
            return;
        };
        let path = view.itemAtRow(view.clickedRow()).and_then(|item| path_of(&item));
        let entries = self.menu_opens(path.as_deref(), &selected_paths(&view));
        let entries = entries.into_iter().filter(|entry| {
            !matches!(
                entry,
                MenuEntry::Item {
                    command: Command::OpenBeside | Command::OpenBelow,
                    ..
                }
            )
        });
        for entry in entries {
            let MenuEntry::Item {
                title,
                command,
                enabled,
            } = entry
            else {
                menu.addItem(&separator(mtm));
                continue;
            };
            let entry = item(mtm, title, Some(action(command)), "");
            set_target(&entry, self);
            entry.setEnabled(enabled);
            menu.addItem(&entry);
        }
        if let Some(entry) = path
            .as_deref()
            .and_then(|path| term_ui_editor::open_in_splat_item(mtm, path))
        {
            menu.addItem(&separator(mtm));
            menu.addItem(&entry);
        }
        menu.setAutoenablesItems(false);
    }
}

/// The paths of the selected rows, top to bottom.
fn selected_paths(view: &NSOutlineView) -> Vec<PathBuf> {
    let rows = view.selectedRowIndexes();
    let mut paths = Vec::new();
    let mut row = rows.firstIndex();
    while row != objc2_foundation::NSNotFound.cast_unsigned() {
        if let Some(path) = isize::try_from(row)
            .ok()
            .and_then(|row| view.itemAtRow(row))
            .and_then(|item| path_of(&item))
        {
            paths.push(path);
        }
        row = rows.indexGreaterThanIndex(row);
    }
    paths
}

/// The action of the menu item for `command`.
fn action(command: Command) -> Sel {
    match command {
        Command::OpenBeside | Command::OpenBelow => sel!(termRowDoubleClicked:),
        Command::OpenAsPreview => sel!(termOpenAsPreview:),
        Command::OpenInBrowserTab => sel!(termOpenInBrowserTab:),
        Command::NewFile => sel!(termNewFile:),
        Command::NewFolder => sel!(termNewFolder:),
        Command::Reveal => sel!(termRevealInFinder:),
        Command::CopyPath => sel!(termCopyPath:),
        Command::CopyRelativePath => sel!(termCopyRelativePath:),
        Command::Rename => sel!(termRename:),
        Command::Trash => sel!(termMoveToTrash:),
    }
}

/// Whether the click that is being handled adds to the selection, with Shift or Command, rather than opening a file.
fn extends_selection(mtm: MainThreadMarker) -> bool {
    let event = NSApplication::sharedApplication(mtm).currentEvent();
    event.is_some_and(|event| {
        event
            .modifierFlags()
            .intersects(objc2_app_kit::NSEventModifierFlags::Shift | objc2_app_kit::NSEventModifierFlags::Command)
    })
}

pub(crate) fn path_of(item: &AnyObject) -> Option<PathBuf> {
    RowItems::<PathBuf>::key_of(item)
}

/// Points the chevron of the folder that opened or closed down or right.
fn turn_chevron(notification: &NSNotification, open: bool) {
    let Some(view) = notification
        .object()
        .and_then(|object| object.downcast::<NSOutlineView>().ok())
    else {
        return;
    };
    let Some(item) = notification
        .userInfo()
        .and_then(|info| info.objectForKey(&ns("NSObject")))
    else {
        return;
    };
    // SAFETY: The item came from this outline view's notification, so the view knows it.
    let row = unsafe { view.rowForItem(Some(&item)) };
    let cell = (row >= 0)
        .then(|| view.viewAtColumn_row_makeIfNecessary(0, row, false))
        .flatten()
        .and_then(|cell| cell.downcast::<NSTableCellView>().ok());
    // SAFETY: The cell is one this tree made, whose image view is the icon it set.
    if let Some(image) = cell.and_then(|cell| unsafe { cell.imageView() }) {
        image.setImage(term_ui::kit::file_outline::chevron(open).as_deref());
    }
}

fn expanded_path(notification: &NSNotification) -> Option<PathBuf> {
    let info = notification.userInfo()?;
    let item = info.objectForKey(&ns("NSObject"))?;
    path_of(&item)
}

/// Whether the event being handled holds Option, which opens a folder with all the folders inside.
fn option_held(mtm: MainThreadMarker) -> bool {
    let event = NSApplication::sharedApplication(mtm).currentEvent();
    event.is_some_and(|event| {
        event
            .modifierFlags()
            .contains(objc2_app_kit::NSEventModifierFlags::Option)
    })
}

/// The row of a folder whose entries take a while to read.
fn spinner_view(mtm: MainThreadMarker) -> Retained<NSView> {
    let cell = NSTableCellView::new(mtm);
    let spinner = NSProgressIndicator::new(mtm);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(objc2_app_kit::NSControlSize::Small);
    cell.addSubview(&spinner);
    spinner.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        spinner.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
        spinner.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]);
    // SAFETY: A nil sender is what AppKit's own callers pass.
    unsafe { spinner.startAnimation(None) };
    cell.into_super()
}

/// Shows `path` in `cell`: its icon, or the chevron of a folder that is `open` or not, and its name.
fn fill_row(cell: &NSTableCellView, path: &Path, ignored: bool, open: Option<bool>) {
    let mtm = cell.mtm();
    // SAFETY: The cell is one `row_view` made, whose image view and text field it set.
    let (Some(image), Some(label)) = (unsafe { cell.imageView() }, unsafe { cell.textField() }) else {
        return;
    };
    term_ui_icons::show_scaled(&image, path, open.is_some(), ui_size::scaled(1.0));
    if let Some(open) = open {
        image.setImage(term_ui::kit::file_outline::chevron(open).as_deref());
        // At the start of the icon's place, where the search panel's chevrons sit under the panel's title.
        let rtl = NSApplication::sharedApplication(mtm).userInterfaceLayoutDirection()
            == NSUserInterfaceLayoutDirection::RightToLeft;
        image.setImageAlignment(if rtl {
            NSImageAlignment::AlignRight
        } else {
            NSImageAlignment::AlignLeft
        });
    } else {
        image.setImageAlignment(NSImageAlignment::AlignCenter);
    }
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    label.setStringValue(&ns(&name));
    label.setTextColor(Some(&*if ignored {
        NSColor::tertiaryLabelColor()
    } else {
        NSColor::labelColor()
    }));
    cell.setToolTip(Some(&ns(&path.to_string_lossy())));
}

/// The side of a row's icon at the default interface size.
const ICON_SIDE: f64 = 16.0;
/// The room between a row's icon and its name at the default interface size.
const ICON_GAP: f64 = 5.0;

/// An empty row for `fill_row`, which the outline view keeps for reuse.
fn row_view(mtm: MainThreadMarker) -> Retained<NSTableCellView> {
    let cell = NSTableCellView::new(mtm);
    cell.setIdentifier(Some(&ns(ROW)));
    let image: Retained<NSImageView> = term_ui_icons::image_view(mtm, Path::new(""), true);
    let label = NSTextField::labelWithString(&ns(""), mtm);
    ui_size::set_system_font(&label, term_ui::kit::style::BODY);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    cell.addSubview(&image);
    cell.addSubview(&label);
    // SAFETY: Both views are subviews of the cell, which keeps them.
    unsafe {
        cell.setImageView(Some(&image));
        cell.setTextField(Some(&label));
    }
    auto_layout(&[&image, &label]);
    let (width, height) = (
        image.widthAnchor().constraintEqualToConstant(ICON_SIDE),
        image.heightAnchor().constraintEqualToConstant(ICON_SIDE),
    );
    let gap = label
        .leadingAnchor()
        .constraintEqualToAnchor_constant(&image.trailingAnchor(), ICON_GAP);
    ui_size::follow_length(&width, ICON_SIDE);
    ui_size::follow_length(&height, ICON_SIDE);
    ui_size::follow_length(&gap, ICON_GAP);
    activate([
        image.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
        image.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        width,
        height,
        gap,
        label
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor(&cell.trailingAnchor()),
        label.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]);
    cell
}

impl WorkspacePart for FileTreeSource {
    fn workspace(&self) -> Option<Workspace> {
        Workspace::open(self.ivars().workspace)
    }

    fn root(&self) -> &Path {
        self.ivars().project.root()
    }
}
