//! Dragging a workspace's row to another place in the list; rows marked fixed, the Terminal, keep their place at the
//! top and cannot be dragged or dropped above.

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSDragOperation, NSDraggingInfo, NSPasteboardItem, NSPasteboardWriting, NSTableView,
    NSTableViewDraggingDestinationFeedbackStyle, NSTableViewDropOperation,
};
use objc2_foundation::NSArray;
use term_ui::kit::ns;

use crate::view::Sidebar;

/// The pasteboard type of a dragged row, holding the workspace's id.
const ROW_TYPE: &str = "term.sidebar.workspace";

/// What the sidebar calls with a workspace's id and the place the user dragged its row to.
pub(crate) type Moved = Box<dyn Fn(u64, usize)>;

impl Sidebar {
    /// Lets the user drag the rows into another order, which `moved` hears as the workspace's id and its new place.
    pub fn on_move(&self, moved: impl Fn(u64, usize) + 'static) {
        *self.ivars().moved.borrow_mut() = Some(Box::new(moved));
        if let Some(table) = self.table() {
            table.registerForDraggedTypes(&NSArray::from_retained_slice(&[ns(ROW_TYPE)]));
            table.setDraggingDestinationFeedbackStyle(NSTableViewDraggingDestinationFeedbackStyle::Gap);
        }
    }

    /// What a drag of table row `row` carries: its workspace's id, unless the row is fixed or its name being edited.
    pub(crate) fn drag_writer(&self, row: isize) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
        if self.ivars().moved.borrow().is_none() {
            return None;
        }
        let row = usize::try_from(row).ok()?;
        if self.editing_row() == Some(row) {
            return None;
        }
        let entry = self.entries().get(row).filter(|entry| !entry.fixed)?.clone();
        let item = NSPasteboardItem::new();
        item.setString_forType(&ns(&entry.id.to_string()), &ns(ROW_TYPE));
        Some(ProtocolObject::from_retained(item))
    }

    /// Where a dragged row may land: between rows, below the fixed ones.
    pub(crate) fn validate_drop(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
        operation: NSTableViewDropOperation,
    ) -> NSDragOperation {
        if dragged(info).is_none() {
            return NSDragOperation::None;
        }
        let place = self.drop_place(row);
        if operation != NSTableViewDropOperation::Above || usize::try_from(row).ok() != Some(place) {
            table.setDropRow_dropOperation(isize::try_from(place).unwrap_or(0), NSTableViewDropOperation::Above);
        }
        NSDragOperation::Move
    }

    /// Moves the dragged row to table row `row`'s place and tells the app; whether it took the drop.
    pub(crate) fn accept_drop(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
    ) -> bool {
        let Some(id) = dragged(info) else {
            return false;
        };
        self.move_row(Some(table), id, self.drop_place(row))
    }

    /// Moves workspace `id`'s row to stand before table row `before`, as a drop there does; whether it was in the list.
    pub(crate) fn move_row(&self, table: Option<&NSTableView>, id: u64, before: usize) -> bool {
        let Some(from) = self.row_of(id) else {
            return false;
        };
        let before = before.max(self.fixed_rows());
        let to = if from < before { before - 1 } else { before };
        if to != from {
            {
                let mut entries = self.ivars().entries.borrow_mut();
                let entry = entries.remove(from);
                let to = to.min(entries.len());
                entries.insert(to, entry);
            }
            if let Some(table) = table {
                table.moveRowAtIndex_toIndex(isize::try_from(from).unwrap_or(0), isize::try_from(to).unwrap_or(0));
            }
            if let Some(moved) = self.ivars().moved.borrow().as_ref() {
                moved(id, to);
            }
        }
        true
    }

    /// Drags workspace `id`'s row before table row `before` as a drop does, for the self-test.
    pub fn drop_row(&self, id: u64, before: usize) -> bool {
        let table = self.table();
        self.move_row(table.as_deref(), id, before)
    }

    /// The point in its window `fraction` of table row `row`'s height down its middle, for the self-test's mouse.
    pub fn row_point(&self, row: usize, fraction: f64) -> Option<objc2_foundation::NSPoint> {
        let table = self.table()?;
        let rect = table.rectOfRow(isize::try_from(row).ok()?);
        let y = if table.isFlipped() {
            rect.origin.y + rect.size.height * fraction
        } else {
            rect.origin.y + rect.size.height * (1.0 - fraction)
        };
        let local = objc2_foundation::NSPoint::new(rect.origin.x + rect.size.width / 2.0, y);
        Some(table.convertPoint_toView(local, None))
    }

    /// The table row a drop at `row` lands before: none of the fixed rows at the top.
    fn drop_place(&self, row: isize) -> usize {
        let rows = self.entries().len();
        usize::try_from(row).unwrap_or(0).clamp(self.fixed_rows(), rows)
    }

    fn fixed_rows(&self) -> usize {
        self.entries().iter().take_while(|entry| entry.fixed).count()
    }
}

/// The id of the workspace whose row `info` drags.
fn dragged(info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<u64> {
    info.draggingPasteboard()
        .stringForType(&ns(ROW_TYPE))?
        .to_string()
        .parse()
        .ok()
}
