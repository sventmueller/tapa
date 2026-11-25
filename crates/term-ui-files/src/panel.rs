//! The file list of a workspace as the sidebar shows it: the buttons of its title row, a new file, a new folder and
//! folding every folder, as VS Code's explorer has them, and the folder tree in a scroll view at the sidebar's width.

use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, sel};
use objc2_app_kit::{NSMenu, NSOutlineView, NSStackView, NSView};
use objc2_foundation::NSObjectProtocol;
use splat_i18n::t;
use term_project::TreeOptions;
use term_ui::kit::panel::{header_button, header_row, source_list_outline};
use term_ui::kit::registry::Registry;
use term_ui::kit::{set_action, style, ui_size};
use term_ui::workspace_host::Workspace;

use crate::project::{Listing, Project};
use crate::view::{FileTreeSource, Pending};

/// The file lists of the open workspaces.
#[derive(Default)]
struct State {
    panels: Registry<Retained<FileTreeSource>>,
}

thread_local! {
    static STATE: Rc<State> = Rc::new(State::default());
}

fn state() -> Rc<State> {
    STATE.with(Rc::clone)
}

/// The list of `workspace`, built for its base path the first time; `None` once the workspace closed or its folder
/// cannot be read.
pub(crate) fn attach(workspace: Workspace) -> Option<Retained<FileTreeSource>> {
    let state = state();
    if let Some(panel) = state.panels.of(workspace) {
        return Some(panel);
    }
    let root = workspace.project_root()?;
    let options = TreeOptions {
        show_ignored: crate::host::installed().is_some_and(|host| host.show_ignored()),
    };
    let id = state.panels.next_id();
    let project = Project::open(&root, id, options)
        .inspect_err(|error| term_base::diag::report("reading a workspace's folder", error))
        .ok()?;
    let source = build(workspace.mtm(), id, workspace, project);
    state.panels.insert(id, source.clone());
    Some(source)
}

/// Lets go of the list of `workspace` and its watcher.
pub(crate) fn detach(workspace: Workspace) {
    state().panels.detach(workspace);
}

/// The list of `workspace`, if it is built.
pub(crate) fn of(workspace: Workspace) -> Option<Retained<FileTreeSource>> {
    state().panels.of(workspace)
}

/// The lists of every open workspace.
pub(crate) fn all() -> Vec<Retained<FileTreeSource>> {
    state().panels.live()
}

/// Runs `f` with the list `id`, if it is still open.
pub(crate) fn with(id: u64, f: impl FnOnce(&FileTreeSource)) {
    state().panels.with(id, f);
}

/// Shows the entries the tree of the list `id` read of the folders in `listings`.
pub(crate) fn listed(id: u64, listings: &[Listing]) {
    with(id, |source| {
        let Some(view) = source.outline() else { return };
        for (dir, entries) in listings {
            source.listed(&view, dir, entries.as_ref());
        }
        source.forget_rows();
    });
}

/// How far the folder tree reaches past the page's leading edge, so that its icons start below the title.
const TREE_OUTDENT: f64 = 6.0;

/// The source with its tree view in a scroll view on the page the sidebar shows below the title row.
fn build(mtm: MainThreadMarker, id: u64, workspace: Workspace, project: Project) -> Retained<FileTreeSource> {
    let source = FileTreeSource::new(mtm, id, workspace.id(), project);
    let view: Retained<NSOutlineView> = Retained::into_super(term_ui::kit::file_outline::FileOutline::new(
        mtm,
        term_ui::kit::panel::LIST_FRAME,
    ));
    let scroll = source_list_outline(mtm, &view, "name", t("Dateien"));
    view.setUsesAutomaticRowHeights(false);
    view.setAllowsMultipleSelection(true);
    ui_size::follow_rows(&view, style::ROW_HEIGHT);
    // SAFETY: The outline view holds its data source and delegate weakly; the registry keeps the source alive.
    unsafe {
        view.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        view.setDelegate(Some(ProtocolObject::from_ref(&*source)));
    }
    set_action(&view, &source, sel!(termRowClicked:));
    assert!(source.respondsToSelector(sel!(termRowDoubleClicked:)));
    // SAFETY: The source answers the double action with a method that takes the sender, checked above.
    unsafe { view.setDoubleAction(Some(sel!(termRowDoubleClicked:))) };
    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*source)));
    // SAFETY: The view keeps the menu; its delegate, the source, lives as long as the view through the registry.
    unsafe { view.setMenu(Some(&menu)) };
    *source.ivars().outline.borrow_mut() = Weak::from(&*view);
    // `Project::open` started reading the root.
    let root = source.ivars().project.root().to_path_buf();
    let waiting = Pending {
        spinner: None,
        deep: false,
    };
    source.ivars().pending.borrow_mut().insert(root.clone(), waiting);
    source.spin_later(root);
    *source.ivars().hover.borrow_mut() = Some(crate::hover::TreeHover::new(mtm, &view));
    let page = NSView::new(mtm);
    page.addSubview(&scroll);
    // The source list indents its rows more than the other panels indent theirs.
    term_ui::kit::pin_inset(
        &scroll,
        &page,
        objc2_foundation::NSEdgeInsets {
            top: 0.0,
            left: -TREE_OUTDENT,
            bottom: 0.0,
            right: 0.0,
        },
    );
    *source.ivars().page.borrow_mut() = Some(page);
    *source.ivars().buttons.borrow_mut() = Some(Retained::into_super(buttons(mtm, &source)));
    source
}

/// The buttons of the title row, sending their actions to `target`, the tree's data source: a new file and a new
/// folder, created in the selected folder or the base path, and folding every folder.
fn buttons(mtm: MainThreadMarker, target: &AnyObject) -> Retained<NSStackView> {
    let row = header_row(mtm);
    for (symbol, name, action) in [
        ("doc.badge.plus", t("Neue Datei …"), sel!(termTreeNewFile:)),
        ("folder.badge.plus", t("Neuer Ordner …"), sel!(termTreeNewFolder:)),
        (
            "rectangle.compress.vertical",
            t("Alle einklappen"),
            sel!(termTreeCollapseAll:),
        ),
    ] {
        row.addArrangedSubview(&header_button(mtm, symbol, name, Some(target), action));
    }
    row
}

impl FileTreeSource {
    /// The page the sidebar shows.
    pub(crate) fn page(&self) -> Option<Retained<NSView>> {
        self.ivars().page.borrow().clone()
    }

    /// The buttons of the title row.
    pub(crate) fn buttons(&self) -> Option<Retained<NSView>> {
        self.ivars().buttons.borrow().clone()
    }
}
