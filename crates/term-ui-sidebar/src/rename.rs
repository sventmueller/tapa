//! Renaming a workspace in its row: its pencil, its menu or a click on the selected workspace's name turn the name
//! into a field; Return keeps what the user typed, Escape the name it had, and the Terminal keeps its name.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, sel};
use objc2_app_kit::{NSResponder, NSTableView, NSTextField, NSView};
use term_ui::kit::ns;

use crate::cell::Cell;
use crate::view::Sidebar;

/// The name the user edits: whose it is, its field and the name it had.
pub(crate) struct Editing {
    id: u64,
    field: Retained<NSTextField>,
    before: String,
}

impl Sidebar {
    /// Has `renamed` hear the name the user gives a workspace in its row; an empty one gives it its folder's again.
    pub fn on_rename(&self, renamed: impl Fn(u64, String) + 'static) {
        *self.ivars().renamed.borrow_mut() = Some(Box::new(renamed));
    }

    /// Turns the name in workspace `id`'s row into a field with the name selected, unless it is the Terminal.
    pub fn begin_rename(&self, id: u64) {
        if self
            .ivars()
            .editing
            .borrow()
            .as_ref()
            .is_some_and(|editing| editing.id == id)
        {
            return;
        }
        let fixed = self
            .entries()
            .iter()
            .find(|entry| entry.id == id)
            .is_none_or(|entry| entry.fixed);
        let (Some(table), Some(row)) = (self.table(), self.row_of(id)) else {
            return;
        };
        if fixed {
            return;
        }
        let row = isize::try_from(row).unwrap_or(0);
        table.scrollRowToVisible(row);
        let Some(field) = name_field(&table, row, true) else {
            return;
        };
        let Some(window) = table.window() else { return };
        field.setEditable(true);
        field.setSelectable(true);
        // SAFETY: The field holds its delegate weakly; the sidebar outlives its table and the table's fields.
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        let before = field.stringValue().to_string();
        *self.ivars().editing.borrow_mut() = Some(Editing {
            id,
            field: field.clone(),
            before,
        });
        // A text field that takes the keyboard selects all it holds.
        window.makeFirstResponder(Some(&field));
    }

    /// The field lost the keyboard: what it holds becomes the name, and it is a label again.
    pub(crate) fn end_rename(&self) {
        let Some(editing) = self.ivars().editing.borrow_mut().take() else {
            return;
        };
        editing.field.setEditable(false);
        editing.field.setSelectable(false);
        // SAFETY: Clearing the delegate leaves the field without one, which it allows.
        unsafe { editing.field.setDelegate(None) };
        let name = editing.field.stringValue().to_string();
        if name != editing.before
            && let Some(renamed) = self.ivars().renamed.borrow().as_ref()
        {
            renamed(editing.id, name);
        }
    }

    /// Escape gives the field the name it had back and ends editing; whether `command` was that.
    pub(crate) fn rename_command(&self, command: Sel) -> bool {
        if command != sel!(cancelOperation:) {
            return false;
        }
        let editing = self
            .ivars()
            .editing
            .borrow()
            .as_ref()
            .map(|editing| (editing.field.clone(), editing.before.clone()));
        if let (Some((field, before)), Some(table)) = (editing, self.table()) {
            field.setStringValue(&ns(&before));
            if let Some(window) = table.window() {
                window.makeFirstResponder(Some(&table));
            }
        }
        true
    }

    /// The table row whose name the user edits.
    pub(crate) fn editing_row(&self) -> Option<usize> {
        let id = self.ivars().editing.borrow().as_ref()?.id;
        self.row_of(id)
    }

    /// Whether a click on `responder` in `table` is one on the name of the selected workspace, which then turns into
    /// a field unless `clicked` says the mouse went on to drag the row.
    pub(crate) fn rename_on_click(
        &self,
        table: &NSTableView,
        responder: &NSResponder,
        clicked: impl FnOnce() -> bool,
    ) -> bool {
        if self.ivars().editing.borrow().is_some() {
            return false;
        }
        let object: &AnyObject = responder;
        let Some(field) = object.downcast_ref::<NSTextField>() else {
            return false;
        };
        let row = table.rowForView(field);
        if row < 0 || row != table.selectedRow() {
            return false;
        }
        let is_name = name_field(table, row, false).is_some_and(|name| std::ptr::eq(&*name, field));
        let id = usize::try_from(row).ok().and_then(|row| {
            self.entries()
                .get(row)
                .filter(|entry| !entry.fixed)
                .map(|entry| entry.id)
        });
        match id.filter(|_| is_name && clicked()) {
            Some(id) => {
                self.begin_rename(id);
                true
            }
            None => false,
        }
    }
}

/// The name field of table row `row`, made if `make` and the row has no view yet.
fn name_field(table: &NSTableView, row: isize, make: bool) -> Option<Retained<NSTextField>> {
    let view: Retained<NSView> = table.viewAtColumn_row_makeIfNecessary(0, row, make)?;
    let cell = view.downcast::<Cell>().ok()?;
    cell.name()
}
