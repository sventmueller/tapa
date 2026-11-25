//! The rows of the Git list: section headers with their "all" buttons, files with status letters and buttons that show
//! on hover; and the list itself, which lines its rows up as VS Code does and takes keyboard commands. The graph's
//! rows are in `history`.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityCustomAction, NSButton, NSColor, NSEvent, NSEventModifierFlags, NSFont,
    NSLineBreakMode, NSOutlineView, NSStackView, NSTableCellView, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};

use super::hover::HoverCell;
use super::model::{FileRow, Mark, RowKey, Section};
use super::panel::{GitPanel, with_panel};
use term_ui::kit::ui_size::{font_like, scaled, system};
use term_ui::kit::{activate, center_y, label, ns, symbol_button};

const SECTION_HEIGHT: f64 = 26.0;
const ACTION_POINTS: f64 = 12.0;

/// Return and the Return of the number block.
const RETURN_KEYS: [u16; 2] = [36, 76];
const SPACE_KEY: u16 = 49;
const DELETE_KEY: u16 = 51;

/// The height of the row for `key`.
pub(crate) fn height(key: Option<&RowKey>) -> f64 {
    scaled(match key {
        Some(RowKey::Section(_)) => SECTION_HEIGHT,
        _ => term_ui::kit::style::ROW_HEIGHT,
    })
}

/// How far AppKit's source list sets its disclosure triangles in past the panel's title.
const OUTDENT: f64 = 5.5;

define_class!(
    // SAFETY: NSOutlineView has no subclassing requirements and `GitOutlineView` does not implement Drop.
    /// Return shows the diff, Space stages or unstages, ⌘⌫ discards.
    #[unsafe(super(NSOutlineView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitOutlineView"]
    pub(crate) struct GitOutlineView;

    unsafe impl NSObjectProtocol for GitOutlineView {}

    impl GitOutlineView {
        /// The disclosure triangle where the panel's title begins, not in the source list's margin; a row below a
        /// section has its triangle where the section has its own, and a commit has none, as its dot is there.
        /// AppKit makes no disclosure button, whose observers would keep it for good; the row's own chevron stands
        /// where it would be.
        #[unsafe(method(frameOfOutlineCellAtRow:))]
        fn no_outline_cell(&self, _row: isize) -> NSRect {
            NSRect::ZERO
        }

        #[unsafe(method(didAddRowView:forRow:))]
        fn did_add_row_view(&self, row_view: &objc2_app_kit::NSTableRowView, row: isize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), didAddRowView: row_view, forRow: row] };
            term_ui::kit::guarded((), || {
                // SAFETY: This class's own method, with its signature.
                let frame: NSRect = unsafe { msg_send![self, termDisclosureFrameAtRow: row] };
                term_ui::kit::panel::chevron::place(self, row_view, frame);
            });
        }

        #[unsafe(method(termDisclosureFrameAtRow:))]
        fn outline_cell_frame(&self, row: isize) -> NSRect {
            // SAFETY: The superclass's implementation, with this method's signature.
            let frame: NSRect = unsafe { msg_send![super(self), frameOfOutlineCellAtRow: row] };
            term_ui::kit::guarded(frame, || {
                let frame = term_ui::kit::panel::outdent(self, frame, OUTDENT, false);
                match self.key_at(row) {
                    Some(RowKey::Commit(_)) => NSRect::ZERO,
                    _ if self.levelForRow(row) > 0 => self.toward_leading(frame, false),
                    _ => frame,
                }
            })
        }

        /// The rows moved along with the triangles: a section's files begin where its triangle does, the graph's
        /// rows below the section's triangle.
        #[unsafe(method(frameOfCellAtColumn:row:))]
        fn cell_frame(&self, column: isize, row: isize) -> NSRect {
            // SAFETY: The superclass's implementation, with this method's signature.
            let frame: NSRect = unsafe { msg_send![super(self), frameOfCellAtColumn: column, row: row] };
            if column != 0 {
                return frame;
            }
            term_ui::kit::guarded(frame, || self.placed_cell(frame, row))
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = guarded_key(self, event);
            if !handled {
                // SAFETY: The superclass's implementation, with this method's signature.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }
    }
);

fn guarded_key(view: &GitOutlineView, event: &NSEvent) -> bool {
    term_ui::kit::guarded(false, || {
        let Some(panel) = view.panel() else { return false };
        let code = event.keyCode();
        let command = event.modifierFlags().contains(NSEventModifierFlags::Command);
        if RETURN_KEYS.contains(&code) {
            panel.activate_row(view.selectedRow());
        } else if code == SPACE_KEY {
            panel.toggle_stage(panel.selected_keys());
        } else if code == DELETE_KEY && command {
            panel.discard(panel.selected_keys());
        } else {
            return false;
        }
        true
    })
}

impl GitOutlineView {
    pub(crate) fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<GitOutlineView> {
        // SAFETY: NSOutlineView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![GitOutlineView::alloc(mtm), initWithFrame: frame] }
    }

    /// The first column's cell `frame` of `row` moved along with the triangles.
    fn placed_cell(&self, frame: NSRect, row: isize) -> NSRect {
        let frame = term_ui::kit::panel::outdent(self, frame, OUTDENT, true);
        let frame = if self.levelForRow(row) <= 0 {
            frame
        } else {
            match self.key_at(row) {
                Some(RowKey::Commit(_) | RowKey::CommitFile(..) | RowKey::More) => self.under_chevron(frame, row),
                Some(RowKey::File(..)) => self.flush_with_chevron(frame, row),
                _ => self.toward_leading(frame, true),
            }
        };
        self.far_edge_at_margin(frame)
    }

    fn key_at(&self, row: isize) -> Option<RowKey> {
        self.itemAtRow(row).and_then(|item| super::panel::key_of(&item))
    }

    /// `frame` moved one level toward the leading edge; a cell keeps its far edge.
    fn toward_leading(&self, frame: NSRect, cell: bool) -> NSRect {
        let by = self.indentationPerLevel();
        let rtl = self.userInterfaceLayoutDirection() == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let x = match (rtl, cell) {
            (false, _) => (frame.origin.x - by).max(0.0),
            (true, false) => frame.origin.x + by,
            (true, true) => frame.origin.x,
        };
        let width = if cell { frame.size.width + by } else { frame.size.width };
        NSRect::new(NSPoint::new(x, frame.origin.y), NSSize::new(width, frame.size.height))
    }

    /// `frame` ending at the panel's margin, so that counts and status letters line up with the message box above.
    fn far_edge_at_margin(&self, frame: NSRect) -> NSRect {
        let width = self.bounds().size.width;
        let rtl = self.userInterfaceLayoutDirection() == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let (left, right) = if rtl {
            (super::ui::MARGIN, frame.origin.x + frame.size.width)
        } else {
            (frame.origin.x, width - super::ui::MARGIN)
        };
        NSRect::new(
            NSPoint::new(left, frame.origin.y),
            NSSize::new((right - left).max(0.0), frame.size.height),
        )
    }

    /// The chevron of the section that holds `row`, if it shows one.
    fn section_chevron(&self, row: isize) -> Option<NSRect> {
        let mut item = self.itemAtRow(row)?;
        // SAFETY: The items come from this outline view, which knows them.
        let section = unsafe {
            while let Some(parent) = self.parentForItem(Some(&item)) {
                item = parent;
            }
            self.rowForItem(Some(&item))
        };
        // SAFETY: This class's own method, with its signature.
        let chevron: NSRect = unsafe { msg_send![self, termDisclosureFrameAtRow: section] };
        (chevron.size.width > 0.0).then_some(chevron)
    }

    /// The cell of a section's file at `row`, beginning at the leading edge of the section's chevron.
    fn flush_with_chevron(&self, frame: NSRect, row: isize) -> NSRect {
        let Some(chevron) = self.section_chevron(row) else {
            return self.toward_leading(frame, true);
        };
        let rtl = self.userInterfaceLayoutDirection() == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let (left, right) = if rtl {
            (frame.origin.x, chevron.origin.x + chevron.size.width)
        } else {
            (chevron.origin.x, frame.origin.x + frame.size.width)
        };
        NSRect::new(
            NSPoint::new(left, frame.origin.y),
            NSSize::new((right - left).max(0.0), frame.size.height),
        )
    }

    /// The cell of a graph row at `row`, beginning so that the first lane runs below its section's chevron.
    fn under_chevron(&self, frame: NSRect, row: isize) -> NSRect {
        let Some(chevron) = self.section_chevron(row) else {
            return frame;
        };
        let middle = chevron.origin.x + chevron.size.width / 2.0;
        let rtl = self.userInterfaceLayoutDirection() == objc2_app_kit::NSUserInterfaceLayoutDirection::RightToLeft;
        let (left, right) = if rtl {
            (frame.origin.x, middle + super::graph::lane() / 2.0)
        } else {
            (middle - super::graph::lane() / 2.0, frame.origin.x + frame.size.width)
        };
        NSRect::new(
            NSPoint::new(left, frame.origin.y),
            NSSize::new((right - left).max(0.0), frame.size.height),
        )
    }

    fn panel(&self) -> Option<Retained<GitPanel>> {
        // SAFETY: Only `ui::outline` makes this view, and it sets the panel as the data source.
        let source = unsafe { self.dataSource() }?;
        let object: Retained<AnyObject> = source.into();
        object.downcast::<GitPanel>().ok()
    }
}

impl GitPanel {
    pub(crate) fn row_view(&self, key: &RowKey) -> Option<Retained<NSView>> {
        let state = self.read();
        let mtm = self.mtm();
        match key {
            RowKey::Section(section) => Some(section_cell(mtm, self, *section, state.model.len(*section))),
            RowKey::File(section, path) => {
                let row = state.model.file(*section, path)?.clone();
                drop(state);
                Some(self.file_cell(mtm, *section, &row))
            }
            RowKey::Commit(hash) => {
                drop(state);
                self.commit_cell(mtm, hash)
            }
            RowKey::CommitFile(hash, path) => {
                drop(state);
                self.commit_file_cell(mtm, hash, path)
            }
            RowKey::More => {
                let lanes = state.model.last_lanes();
                drop(state);
                Some(self.more_cell(mtm, lanes))
            }
            RowKey::Stash(hash) => {
                drop(state);
                self.stash_cell(mtm, hash)
            }
            RowKey::StashFile(hash, file) => {
                drop(state);
                self.stash_file_cell(mtm, hash, file)
            }
        }
    }

    fn file_cell(&self, mtm: MainThreadMarker, section: Section, row: &FileRow) -> Retained<NSView> {
        let cell = HoverCell::new(mtm);
        let (text, name) = file_text(mtm, row.path(), row.mark == Mark::Deleted);
        let letter = letter_label(mtm, row.mark);
        for view in [&*text as &NSView, &letter] {
            cell.addSubview(view);
            center_y(view, &cell);
        }
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&name)) };
        let gap = scaled(4.0);
        activate([
            text.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
            letter.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
        ]);
        let panel = objc2::rc::Weak::from(self);
        let build = move || Some(panel.load()?.row_actions(mtm, section).into_super());
        cell.set_actions(build, &text, &letter, gap, gap);
        let described = describe(section, row);
        cell.setToolTip(Some(&ns(&described)));
        cell.setAccessibilityLabel(Some(&ns(&described)));
        let custom = self.accessibility_actions(section, row);
        cell.setAccessibilityCustomActions(Some(&NSArray::from_retained_slice(&custom)));
        cell.into_super().into_super()
    }

    /// The buttons a file row shows on hover.
    fn row_actions(&self, mtm: MainThreadMarker, section: Section) -> Retained<NSStackView> {
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(scaled(term_ui::kit::style::space::XXS));
        for (name, title, action) in file_actions(section) {
            let button: Retained<NSButton> = symbol_button(mtm, name, title, scaled(ACTION_POINTS), Some(self), action);
            button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            stack.addArrangedSubview(&button);
        }
        stack
    }

    /// The row's actions for VoiceOver, which reaches no hover buttons.
    fn accessibility_actions(&self, section: Section, row: &FileRow) -> Vec<Retained<NSAccessibilityCustomAction>> {
        let mut actions: Vec<(&str, Sel)> = vec![];
        if section != Section::Conflicts {
            actions.push((t("Diff zeigen"), sel!(termGitDiff:)));
        }
        actions.extend(
            file_actions(section)
                .into_iter()
                .map(|(_, title, action)| (title, action)),
        );
        let key = RowKey::File(section, row.path().to_path_buf());
        let entries: Vec<(&str, Option<Sel>)> = actions
            .into_iter()
            .map(|(title, action)| (title, Some(action)))
            .collect();
        self.custom_actions(&key, &entries)
    }

    /// The menu entries of `key` as actions for VoiceOver, which reaches no context menu.
    pub(super) fn custom_actions(
        &self,
        key: &RowKey,
        entries: &[(&str, Option<Sel>)],
    ) -> Vec<Retained<NSAccessibilityCustomAction>> {
        entries
            .iter()
            .filter_map(|(title, action)| Some((*title, (*action)?)))
            .map(|(title, action)| {
                let id = self.id();
                let key = key.clone();
                let handler = RcBlock::new(move || -> Bool {
                    term_ui::kit::guarded((), || with_panel(id, |p| p.run_row_action(action, vec![key.clone()])));
                    Bool::YES
                });
                NSAccessibilityCustomAction::initWithName_handler(
                    NSAccessibilityCustomAction::alloc(),
                    &ns(title),
                    Some(&handler),
                )
            })
            .collect()
    }

    /// Runs the row action `action` on `keys`, as its button or menu item would.
    pub(crate) fn run_row_action(&self, action: Sel, keys: Vec<RowKey>) {
        if action == sel!(termGitStage:) {
            self.stage(keys);
        } else if action == sel!(termGitUnstage:) {
            self.unstage(keys);
        } else if action == sel!(termGitDiscard:) {
            self.discard(keys);
        } else if action == sel!(termGitOpen:) {
            self.open(keys);
        } else if action == sel!(termGitResolve:) {
            self.resolve(keys);
        } else if action == sel!(termGitDiff:)
            && let Some(key) = keys.first()
        {
            self.show_diff(key, false);
        } else if action == sel!(termGitApplyStash:) {
            self.apply_stash(&keys, term_git::Unstash::Apply);
        } else if action == sel!(termGitPopStash:) {
            self.apply_stash(&keys, term_git::Unstash::Pop);
        } else if action == sel!(termGitDropStash:) {
            self.drop_stash(&keys);
        } else {
            self.run_history_action(action, &keys);
        }
    }
}

/// The symbol, title and action of each button of a file row.
fn file_actions(section: Section) -> Vec<(&'static str, &'static str, Sel)> {
    let open = ("doc.text", t("Im Editor öffnen"), sel!(termGitOpen:));
    match section {
        Section::Conflicts => vec![open, ("checkmark", t("Als gelöst markieren"), sel!(termGitResolve:))],
        Section::Staged => vec![open, ("minus", t("Zurücknehmen"), sel!(termGitUnstage:))],
        _ => vec![
            ("arrow.uturn.backward", t("Verwerfen"), sel!(termGitDiscard:)),
            open,
            ("plus", t("Vormerken"), sel!(termGitStage:)),
        ],
    }
}

/// The file's name, folder and state in words, for VoiceOver and the tooltip.
fn describe(section: Section, row: &FileRow) -> String {
    let path = row.path().display().to_string();
    let state = match row.from() {
        Some(from) => tf("{}, von {}", &[row.mark.word(), &from.display().to_string()]),
        None => row.mark.word().to_string(),
    };
    let side = match section {
        Section::Conflicts => t("ungelöst"),
        Section::Staged => t("vorgemerkt"),
        _ => t("nicht vorgemerkt"),
    };
    tf("{}: {}, {}", &[&path, &state, side])
}

/// Colours that keep their meaning in light and dark mode and with more contrast; the letter carries it as well.
pub(super) fn mark_color(mark: Mark) -> Retained<NSColor> {
    match mark {
        Mark::Modified | Mark::TypeChanged => NSColor::systemOrangeColor(),
        Mark::Added | Mark::Untracked => NSColor::systemGreenColor(),
        Mark::Deleted => NSColor::systemRedColor(),
        Mark::Renamed | Mark::Copied => NSColor::systemBlueColor(),
        Mark::Conflict => NSColor::systemPurpleColor(),
    }
}

fn section_cell(mtm: MainThreadMarker, panel: &GitPanel, section: Section, count: usize) -> Retained<NSView> {
    let cell = NSTableCellView::new(mtm);
    let title = term_ui::kit::panel::heading(mtm, section.title());
    title.setContentCompressionResistancePriority_forOrientation(
        100.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    cell.addSubview(&title);
    title.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        title.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
        title.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]);
    let number = pill(
        mtm,
        &count.to_string(),
        &NSColor::quaternaryLabelColor(),
        &NSColor::secondaryLabelColor(),
    );
    number.setHidden(section == Section::Commits);
    cell.addSubview(&number);
    number.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        number.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
        number.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        title
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&number.leadingAnchor(), -scaled(30.0)),
    ]);
    let all = match section {
        Section::Staged => Some(("minus", t("Alle zurücknehmen"), sel!(termGitUnstageAll:))),
        Section::Changes if count > 0 => Some(("plus", t("Alle vormerken"), sel!(termGitStageAll:))),
        _ => None,
    };
    if let Some((name, description, action)) = all {
        let button = symbol_button(mtm, name, description, scaled(ACTION_POINTS), Some(panel), action);
        button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        cell.addSubview(&button);
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        activate([
            button
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&number.leadingAnchor(), -scaled(4.0)),
            button.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        ]);
    }
    let files = match (section, count) {
        (Section::Commits, _) => tf("{}, {} Commits", &[section.title(), &count.to_string()]),
        (Section::Stashes, 1) => tf("{}, 1 Eintrag", &[section.title()]),
        (Section::Stashes, _) => tf("{}, {} Einträge", &[section.title(), &count.to_string()]),
        (_, 1) => tf("{}, 1 Datei", &[section.title()]),
        _ => tf("{}, {} Dateien", &[section.title(), &count.to_string()]),
    };
    cell.setAccessibilityLabel(Some(&ns(&files)));
    // Not the cell's text field: the source list would draw it dimmer than the panel's title.
    cell.into_super()
}

impl GitPanel {
    /// What a click, the context menu or a key acts on: the clicked row, and the whole selection when it includes it.
    pub(crate) fn keys_for(&self, sender: Option<&AnyObject>) -> Vec<RowKey> {
        if sender.is_some_and(|s| s.downcast_ref::<objc2_app_kit::NSMenuItem>().is_some()) {
            return self.read().menu_keys.clone();
        }
        let outline = &self.ui().outline;
        let row = sender
            .and_then(|s| s.downcast_ref::<NSView>())
            .map_or(-1, |view| outline.rowForView(view));
        self.keys_around(row)
    }

    /// The row `row`, or the selection when `row` is part of it or none.
    fn keys_around(&self, row: isize) -> Vec<RowKey> {
        let outline = &self.ui().outline;
        if row >= 0 && !outline.isRowSelected(row) {
            return outline
                .itemAtRow(row)
                .and_then(|i| super::panel::key_of(&i))
                .into_iter()
                .collect();
        }
        self.selected_keys()
    }

    pub(crate) fn selected_keys(&self) -> Vec<RowKey> {
        let outline = &self.ui().outline;
        (0..outline.numberOfRows())
            .filter(|row| outline.isRowSelected(*row))
            .filter_map(|row| outline.itemAtRow(row))
            .filter_map(|item| super::panel::key_of(&item))
            .collect()
    }

    /// A single click: a file shows its diff in the preview tab, unless the click adds to the selection.
    pub(crate) fn preview_row(&self, row: isize) {
        let outline = &self.ui().outline;
        let Some(item) = (row >= 0).then(|| outline.itemAtRow(row)).flatten() else {
            return;
        };
        let modifiers = objc2_app_kit::NSApplication::sharedApplication(self.mtm())
            .currentEvent()
            .map(|event| event.modifierFlags());
        let extends = modifiers.is_some_and(|flags| {
            flags.intersects(objc2_app_kit::NSEventModifierFlags::Shift | objc2_app_kit::NSEventModifierFlags::Command)
        });
        if extends {
            return;
        }
        match super::panel::key_of(&item) {
            Some(key @ (RowKey::File(..) | RowKey::CommitFile(..) | RowKey::StashFile(..))) => {
                self.show_diff(&key, true);
            }
            Some(RowKey::Commit(_) | RowKey::Stash(_)) => self.toggle(&item),
            Some(RowKey::More) => self.read_more(),
            _ => {}
        }
    }

    /// Opens `item` when it is closed, else closes it.
    fn toggle(&self, item: &AnyObject) {
        let outline = &self.ui().outline;
        // SAFETY: The item came from this outline view, which knows it.
        unsafe {
            if outline.isItemExpanded(Some(item)) {
                outline.collapseItem(Some(item));
            } else {
                outline.expandItem(Some(item));
            }
        }
    }

    /// Return or a double click: a file shows its diff, a section opens or closes.
    pub(crate) fn activate_row(&self, row: isize) {
        let outline = &self.ui().outline;
        let Some(item) = (row >= 0).then(|| outline.itemAtRow(row)).flatten() else {
            return;
        };
        match super::panel::key_of(&item) {
            Some(key @ RowKey::File(Section::Conflicts, _)) => self.open(vec![key]),
            Some(key @ (RowKey::File(..) | RowKey::CommitFile(..) | RowKey::StashFile(..))) => {
                self.show_diff(&key, false)
            }
            Some(RowKey::Section(_) | RowKey::Commit(_) | RowKey::Stash(_)) => self.toggle(&item),
            Some(RowKey::More) => self.read_more(),
            None => {}
        }
    }

    /// Space: stages what is unstaged and unstages what is staged; on a section's row, all of its files, as the
    /// section's button does.
    pub(crate) fn toggle_stage(&self, keys: Vec<RowKey>) {
        match keys.as_slice() {
            [RowKey::Section(Section::Changes)] => return self.stage_all(),
            [RowKey::Section(Section::Staged)] => return self.unstage_all(),
            _ => {}
        }
        let (staged, other): (Vec<RowKey>, Vec<RowKey>) = keys
            .into_iter()
            .partition(|k| matches!(k, RowKey::File(Section::Staged, _)));
        if !other.is_empty() {
            self.stage(other);
        }
        if !staged.is_empty() {
            self.unstage(staged);
        }
    }

    /// Reloads the list, keeping which sections are closed and which rows are selected; only the rows that changed,
    /// when the last refresh changed nothing else.
    pub(crate) fn reload_outline(&self) {
        let only = self.state().reload_only.take();
        let away = self.out_of_view();
        self.set_stale(away);
        if away {
            return;
        }
        let outline = &self.ui().outline;
        if let Some(keys) = only {
            for key in keys {
                // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
                unsafe { outline.reloadItem(Some(&self.item_for(&key))) };
            }
            return;
        }
        let selected = self.selected_keys();
        self.cells().keep_shown();
        term_ui::kit::panel::reload(outline);
        let (sections, collapsed) = {
            let state = self.read();
            (state.model.sections(), state.collapsed.clone())
        };
        for section in sections.into_iter().filter(|s| !collapsed.contains(s)) {
            let item = self.item_for(&RowKey::Section(section));
            // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
            unsafe { outline.expandItem(Some(&item)) };
        }
        let (commits, stashes): (Vec<_>, Vec<_>) = {
            let state = self.read();
            let shown = |hash: &&term_git::CommitId| state.model.commit(hash).is_some();
            let stashed = |hash: &&term_git::CommitId| state.model.stash(hash).is_some();
            (
                state.open_commits.iter().filter(shown).cloned().collect(),
                state.open_stashes.iter().filter(stashed).cloned().collect(),
            )
        };
        let keys = commits
            .into_iter()
            .map(RowKey::Commit)
            .chain(stashes.into_iter().map(RowKey::Stash));
        for key in keys {
            let item = self.item_for(&key);
            // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
            unsafe { outline.expandItem(Some(&item)) };
        }
        let rows = objc2_foundation::NSMutableIndexSet::new();
        for key in &selected {
            // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
            let row = unsafe { outline.rowForItem(Some(&self.item_for(key))) };
            if let Ok(row) = usize::try_from(row) {
                rows.addIndex(row);
            }
        }
        outline.selectRowIndexes_byExtendingSelection(&rows, false);
        self.forget_gone_items();
    }

    /// The context menu for the clicked row, or the selection it belongs to.
    pub(crate) fn fill_menu(&self, menu: &objc2_app_kit::NSMenu) {
        menu.removeAllItems();
        menu.setAutoenablesItems(false);
        let keys = self.keys_around(self.ui().outline.clickedRow());
        let entries = menu_entries(&keys, &self.read().model);
        self.state().menu_keys = keys;
        let mtm = self.mtm();
        for (title, action) in entries {
            let Some(action) = action else {
                menu.addItem(&term_ui::kit::separator(mtm));
                continue;
            };
            let entry = term_ui::kit::item(mtm, title, Some(action), "");
            term_ui::kit::set_target(&entry, self);
            menu.addItem(&entry);
        }
    }
}

/// The menu for `keys`: file actions that fit every one of them, or a section's "all" action.
fn menu_entries(keys: &[RowKey], model: &super::model::Model) -> Vec<(&'static str, Option<Sel>)> {
    let files: Vec<(Section, &FileRow)> = keys
        .iter()
        .filter_map(|k| match k {
            RowKey::File(section, path) => model.file(*section, path).map(|row| (*section, row)),
            _ => None,
        })
        .collect();
    if let [RowKey::Section(section)] = keys {
        return match section {
            Section::Staged => vec![(t("Alle zurücknehmen"), Some(sel!(termGitUnstageAll:)))],
            Section::Changes => vec![(t("Alle vormerken"), Some(sel!(termGitStageAll:)))],
            Section::Conflicts | Section::Stashes | Section::Commits => vec![],
        };
    }
    if let [RowKey::Commit(_) | RowKey::CommitFile(..)] = keys {
        return super::history::menu(keys);
    }
    if let [RowKey::Stash(_) | RowKey::StashFile(..)] = keys {
        return super::stashes::menu(keys);
    }
    if files.is_empty() {
        return vec![];
    }
    if files.iter().all(|(s, _)| *s == Section::Conflicts) {
        return vec![
            (t("Im Editor öffnen"), Some(sel!(termGitOpen:))),
            (t("Als gelöst markieren"), Some(sel!(termGitResolve:))),
        ];
    }
    let mut entries = vec![];
    if files.len() == 1 {
        entries.push((t("Diff zeigen"), Some(sel!(termGitDiff:))));
    }
    entries.push((t("Im Editor öffnen"), Some(sel!(termGitOpen:))));
    entries.push(("", None));
    if files.iter().any(|(s, _)| *s == Section::Changes) {
        entries.push((t("Vormerken"), Some(sel!(termGitStage:))));
    }
    if files.iter().any(|(s, _)| *s == Section::Staged) {
        entries.push((t("Zurücknehmen"), Some(sel!(termGitUnstage:))));
    }
    if files.iter().any(|(s, _)| *s == Section::Changes) {
        entries.push(("", None));
        entries.push((t("Verwerfen …"), Some(sel!(termGitDiscard:))));
    }
    entries
}

#[cfg(test)]
mod tests;

/// A file's icon, name and folder in a row; the name is the stack's label for VoiceOver.
pub(super) fn file_text(
    mtm: MainThreadMarker,
    path: &std::path::Path,
    deleted: bool,
) -> (Retained<NSStackView>, Retained<objc2_app_kit::NSTextField>) {
    let icon = term_ui_icons::image_view(mtm, path, false);
    term_ui_icons::show_scaled(&icon, path, false, scaled(1.0));
    icon.widthAnchor()
        .constraintEqualToConstant(scaled(16.0))
        .setActive(true);
    let name = label(
        mtm,
        &path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
    );
    name.setFont(Some(&system(term_ui::kit::style::BODY)));
    name.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    if deleted {
        name.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    let folder = label(
        mtm,
        &path.parent().map_or_else(String::new, |p| p.display().to_string()),
    );
    folder.setFont(Some(&font_like(&term_ui::kit::style::small_system())));
    folder.setTextColor(Some(&NSColor::secondaryLabelColor()));
    folder.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
    folder.setContentCompressionResistancePriority_forOrientation(
        100.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    let text = NSStackView::new(mtm);
    text.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    text.setSpacing(scaled(term_ui::kit::style::space::SNUG));
    for view in [&*icon as &NSView, &name, &folder] {
        text.addArrangedSubview(view);
    }
    (text, name)
}

/// The mark of a file a commit or stash entry changed in `change`'s way.
pub(super) fn change_mark(change: term_git::Change) -> Mark {
    use term_git::Change;
    match change {
        Change::Added => Mark::Added,
        Change::Deleted => Mark::Deleted,
        Change::Renamed { .. } => Mark::Renamed,
        Change::Copied { .. } => Mark::Copied,
        Change::Unmerged => Mark::Conflict,
        Change::Modified => Mark::Modified,
    }
}

/// A file's status letter in its colour.
pub(super) fn letter_label(mtm: MainThreadMarker, mark: Mark) -> Retained<objc2_app_kit::NSTextField> {
    let letter = label(mtm, mark.letter());
    letter.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        scaled(term_ui::kit::style::SMALL),
        term_render::constant!(objc2_app_kit::NSFontWeightBold),
    )));
    letter.setTextColor(Some(&mark_color(mark)));
    letter.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    letter
        .widthAnchor()
        .constraintEqualToConstant(scaled(12.0))
        .setActive(true);
    letter.setContentHuggingPriority_forOrientation(750.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
    letter
}

/// A rounded label in `fill`, as a count or a branch beside a commit.
pub(super) fn pill(mtm: MainThreadMarker, text: &str, fill: &NSColor, color: &NSColor) -> Retained<NSView> {
    pill_with(mtm, None, text, fill, color)
}

/// A pill with the symbol `symbol_name` before its text, or only the symbol when `text` is empty.
pub(super) fn pill_with(
    mtm: MainThreadMarker,
    symbol_name: Option<&str>,
    text: &str,
    fill: &NSColor,
    color: &NSColor,
) -> Retained<NSView> {
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(scaled(term_ui::kit::style::space::XXS));
    let side = scaled(if symbol_name.is_some() { 4.0 } else { 6.0 });
    row.setEdgeInsets(objc2_foundation::NSEdgeInsets {
        top: scaled(1.0),
        left: side,
        bottom: scaled(1.0),
        right: side,
    });
    if let Some(image) = symbol_name.and_then(|name| term_ui::kit::symbol(name, "")) {
        let icon = objc2_app_kit::NSImageView::imageViewWithImage(&image, mtm);
        icon.setSymbolConfiguration(Some(
            &objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(
                scaled(8.0),
                term_render::constant!(objc2_app_kit::NSFontWeightSemibold),
            ),
        ));
        icon.setContentTintColor(Some(color));
        term_ui::kit::hide_from_voiceover(&icon);
        row.addArrangedSubview(&icon);
    }
    if !text.is_empty() {
        let words = label(mtm, text);
        words.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            scaled(NSFont::smallSystemFontSize()),
            term_render::constant!(objc2_app_kit::NSFontWeightMedium),
        )));
        words.setTextColor(Some(color));
        words.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        row.addArrangedSubview(&words);
    }
    let pill = objc2_app_kit::NSBox::new(mtm);
    pill.setBoxType(objc2_app_kit::NSBoxType::Custom);
    pill.setTitlePosition(objc2_app_kit::NSTitlePosition::NoTitle);
    pill.setBorderWidth(0.0);
    pill.setCornerRadius(scaled(7.0));
    pill.setFillColor(fill);
    pill.setContentViewMargins(NSSize::new(0.0, 0.0));
    if let Some(content) = pill.contentView() {
        content.addSubview(&row);
        term_ui::kit::pin_edges(&row, &content);
    }
    pill.setContentHuggingPriority_forOrientation(750.0, objc2_app_kit::NSLayoutConstraintOrientation::Horizontal);
    pill.setContentCompressionResistancePriority_forOrientation(
        740.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    pill.into_super()
}
