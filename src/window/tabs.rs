//! The tabs of the selected workspace and the panes of the tab in front: new tabs in the workspace's base folder,
//! choosing among the tabs of the window's tab group as its tab bar does, moving them, splitting and moving between
//! panes.

use std::path::PathBuf;
use std::rc::Rc;
use term_ui::workspace_host::{TabId, TabKind, WorkspaceId};

use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSAttributedStringAttachmentConveniences, NSTextAttachment};
use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString, NSString};
use term_ui_panes::{Direction, Side};

use super::{Window, session};
use crate::state::app;
use crate::tab::Tab;
use term_ui::kit::ui_size;

impl Window {
    /// Opens a tab after the one in front, in the selected workspace's base folder (E20).
    pub(crate) fn new_tab(self: &Rc<Self>) {
        let base = self.selected_workspace().map(|workspace| workspace.base.clone());
        self.new_tab_in(base);
    }

    /// Makes tab `id` and its workspace the selected ones, so that what opens next opens beside it.
    pub(crate) fn select_beside(&self, id: u64) -> Option<Rc<crate::workspace::Workspace>> {
        let workspace = self.workspace_of_tab(id)?;
        workspace.select_tab(TabId::new(id));
        self.workspaces.borrow_mut().select(WorkspaceId::new(workspace.id));
        Some(workspace)
    }

    /// Opens a tab in `folder` after the one in front of the selected workspace, as a window of its tab group; the
    /// quick terminal keeps its one.
    pub(crate) fn new_tab_in(self: &Rc<Self>, folder: Option<PathBuf>) {
        if self.is_quick() {
            objc2_app_kit::NSBeep();
            return;
        }
        let Some(workspace) = self.selected_workspace() else {
            return;
        };
        let config = app().config();
        let folder = folder
            .filter(|folder| folder.is_dir())
            .unwrap_or_else(|| session::start_folder(&config, None, false));
        let tab = self.make_tab(folder, None);
        let anchor = workspace.tabs.borrow().selected_id().map(TabId::get);
        if let Some(anchor) = anchor.and_then(|id| self.frame_of(id)) {
            self.sync_group_order(&anchor);
        }
        workspace.tabs.borrow_mut().insert_after_selected(tab.clone());
        self.attach(&tab, workspace.id, anchor, None, None);
        self.select(tab.id);
        self.start_sessions(&tab);
        self.refresh();
    }

    /// Brings tab `id` to the front in its window, with its workspace's tab group in place of the one in front when
    /// that group is off screen.
    pub(crate) fn select(self: &Rc<Self>, id: u64) {
        let Some(workspace) = self.workspace_of_tab(id) else {
            return;
        };
        let tab = workspace.tabs.borrow().get(TabId::new(id));
        if let Some(tab) = tab {
            self.attach_restored_frame(&tab);
        }
        let Some(frame) = self.frame_of(id) else {
            return;
        };
        workspace.select_tab(TabId::new(id));
        if frame.is_shown() {
            frame.ns.makeKeyAndOrderFront(None);
            self.ensure_tab_key(id);
        } else {
            let replacing = self.frame_of(self.last_key.get().unwrap_or(id));
            self.show_workspace(workspace.id, replacing);
        }
        self.focus_active();
    }

    /// Brings the tab `step` places from the one in front of its window's tab bar forward, wrapping around.
    pub(crate) fn select_relative(&self, step: isize) {
        let Some(front) = self.front_frame() else { return };
        if step < 0 {
            front.ns.selectPreviousTab(None);
        } else {
            front.ns.selectNextTab(None);
        }
    }

    /// Brings the tab at `index` of the front window's tab bar forward; for none, the last.
    pub(crate) fn select_index(&self, index: Option<usize>) {
        let Some(front) = self.front_frame() else { return };
        let group = front.group();
        let chosen = match index {
            Some(index) => group.get(index),
            None => group.last(),
        };
        if let Some(window) = chosen {
            window.makeKeyAndOrderFront(None);
        }
    }

    /// Moves tab `id` to `index` in its window's tab bar, as the user drags it there, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn move_tab(&self, id: u64, index: usize) {
        let Some(frame) = self.frame_of(id) else { return };
        let Some(group) = frame.ns.tabGroup() else { return };
        let count = group.windows().len();
        if count < 2 {
            return;
        }
        let selected = group.selectedWindow();
        group.removeWindow(&frame.ns);
        group.insertWindow_atIndex(&frame.ns, isize::try_from(index.min(count - 1)).unwrap_or(0));
        if let Some(selected) = selected {
            group.setSelectedWindow(Some(&selected));
        }
        self.sync_group_order(&frame);
        self.refresh();
    }

    // MARK: panes

    /// Splits the active pane, the new session at `side` in the folder of the active one where the settings say so.
    pub(crate) fn split(&self, side: Side) {
        let Some(tab) = self.selected_tab() else { return };
        let Some(terminal) = tab.terminal() else {
            return objc2_app_kit::NSBeep();
        };
        let config = app().config();
        let current = tab.focused_view().map(|view| view.cwd());
        let folder = session::start_folder(&config, current, config.window.inherit_directory.split);
        let (view, root) = session::make(self.mtm, self.id(), folder.clone(), None);
        tab.add_session(view.id(), folder);
        terminal.panes.split(side, view.id(), root);
        self.start_sessions(&tab);
        self.refresh();
    }

    /// Moves the focus `step` panes on in the tab in front.
    pub(crate) fn step_pane(&self, step: isize) {
        if let Some(terminal) = self.selected_tab().as_deref().and_then(Tab::terminal) {
            terminal.panes.step(step);
            self.focus_active();
        }
    }

    /// Lets the active pane fill the tab, or shows all again.
    pub(crate) fn toggle_zoom(&self) {
        if let Some(terminal) = self.selected_tab().as_deref().and_then(Tab::terminal) {
            terminal.panes.toggle_zoom();
            self.focus_active();
        }
    }

    /// Gives the panes of the tab in front equal shares.
    pub(crate) fn equalize(&self) {
        if let Some(terminal) = self.selected_tab().as_deref().and_then(Tab::terminal) {
            terminal.panes.equalize();
            self.focus_active();
        }
    }

    /// Moves the divider beside the active pane.
    pub(crate) fn resize_pane(&self, direction: Direction) {
        if let Some(terminal) = self.selected_tab().as_deref().and_then(Tab::terminal) {
            terminal.panes.resize(direction);
            self.focus_active();
        }
    }
}

/// The height of the picture before a tab's title at the default interface size, as in Dango.
const ICON_HEIGHT: f64 = 14.0;
/// The point size of the symbol before a tab's title at the default interface size, as in Dango.
const ICON_POINTS: f64 = 11.0;

/// What the tab bar shows for `tab`, as Dango titles its tabs: a file's icon, else the symbol
/// of what the tab shows, an agent's in its colour, before the title; all at the interface size.
pub(super) fn tab_title(tab: &Tab, title: &str) -> Retained<NSAttributedString> {
    let text = NSMutableAttributedString::new();
    let font = ui_size::system(objc2_app_kit::NSFont::systemFontSize());
    if let Some(image) = tab_icon(tab, title) {
        let size = image.size();
        let height = ui_size::scaled(ICON_HEIGHT).min(size.height.max(1.0));
        let width = size.width * height / size.height.max(1.0);
        let attachment = NSTextAttachment::new();
        attachment.setImage(Some(&image));
        attachment.setBounds(objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0.0, (font.capHeight() - height) / 2.0),
            objc2_foundation::NSSize::new(width, height),
        ));
        text.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(&attachment));
        text.appendAttributedString(&attributed("\u{2002}", &font));
    }
    text.appendAttributedString(&attributed(title, &font));
    Retained::into_super(text)
}

fn attributed(text: &str, font: &objc2_app_kit::NSFont) -> Retained<NSAttributedString> {
    // SAFETY: NSFontAttributeName is a constant AppKit sets when it loads.
    let key = unsafe { objc2_app_kit::NSFontAttributeName };
    let attributes = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[font as &AnyObject]);
    // SAFETY: The attributes map an attribute name to a font, which the font attribute takes.
    unsafe {
        NSAttributedString::initWithString_attributes(AnyThread::alloc(), &term_ui::kit::ns(text), Some(&attributes))
    }
}

/// The picture before the title of `tab`.
fn tab_icon(tab: &Tab, title: &str) -> Option<Retained<objc2_app_kit::NSImage>> {
    use objc2_app_kit::{NSColor, NSImageSymbolConfiguration};
    let kind = tab.as_hosted().map_or(TabKind::Terminal, |hosted| hosted.kind());
    // A file's tab is named after the file from its start, before its path is known.
    if kind == TabKind::File
        && let Some(image) = term_ui_icons::icon_image(&term_ui_icons::icon_of(title), ui_size::scaled(1.0))
    {
        return Some(image);
    }
    let symbol = match kind {
        TabKind::Terminal => "terminal",
        TabKind::Agent => "sparkle",
        TabKind::Browser => "globe",
        TabKind::File => "doc.text",
        TabKind::Log => "doc.plaintext",
    };
    let (tint, four) = tab.as_hosted().map(|hosted| hosted.colours()).unwrap_or_default();
    let color = match (tint, kind) {
        (Some(tint), _) => tint,
        (None, TabKind::Agent) => term_ui_agent::Agent::Claude.colour(),
        (None, _) => NSColor::secondaryLabelColor(),
    };
    // SAFETY: AppKit sets its exported font weight constants when it loads and never changes them.
    let weight = unsafe { objc2_app_kit::NSFontWeightRegular };
    let tint = NSImageSymbolConfiguration::configurationWithHierarchicalColor(&color);
    let configuration =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(ui_size::scaled(ICON_POINTS), weight)
            .configurationByApplyingConfiguration(&tint);
    let image = term_ui::kit::symbol(symbol, "")?.imageWithSymbolConfiguration(&configuration)?;
    Some(match four {
        Some(colors) => term_ui::kit::four_colours(&image, &colors),
        None => image,
    })
}
