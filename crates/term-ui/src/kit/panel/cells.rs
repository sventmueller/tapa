//! Lists that reload without building the cells of unchanged rows again: on macOS 27 every control moved into a new
//! row view leaves key-value dependencies behind for good, which `reloadData` and `reloadItem(nil, …)` do to every
//! row they show, while reloading rows in place and handing back the same cell does not.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSOutlineView, NSView};
use objc2_foundation::{NSIndexSet, NSRange};

/// How many cells of rows out of view a list keeps.
const KEPT_AWAY: usize = 200;

/// The cells a list built, by the key of their row, with a hash of what each shows.
pub struct KeptCells<K> {
    cells: RefCell<HashMap<K, (u64, Retained<NSView>)>>,
}

impl<K> Default for KeptCells<K> {
    fn default() -> Self {
        KeptCells {
            cells: RefCell::new(HashMap::new()),
        }
    }
}

impl<K: Clone + Eq + Hash> KeptCells<K> {
    /// The cell of `key` built before, while it was built from the same `look`; else the one `build` makes.
    pub fn cell(
        &self,
        key: &K,
        look: &impl Hash,
        build: impl FnOnce() -> Option<Retained<NSView>>,
    ) -> Option<Retained<NSView>> {
        let mut hasher = DefaultHasher::new();
        look.hash(&mut hasher);
        let look = hasher.finish();
        if let Some((kept, cell)) = self.cells.borrow().get(key)
            && *kept == look
        {
            return Some(cell.clone());
        }
        let cell = build()?;
        self.cells.borrow_mut().insert(key.clone(), (look, cell.clone()));
        Some(cell)
    }

    /// Lets go of the cells of rows `live` no longer knows.
    pub fn keep(&self, mut live: impl FnMut(&K) -> bool) {
        self.cells.borrow_mut().retain(|key, _| live(key));
    }

    /// Lets go of the cells no row shows now, such as those scrolled away, past the first [`KEPT_AWAY`]; before a
    /// reload, so memory stays bounded while scrolling back finds most rows built.
    pub fn keep_shown(&self) {
        let mut away = 0;
        self.cells.borrow_mut().retain(|_, (_, cell)| {
            // SAFETY: Asking for the superview only reads.
            let shown = unsafe { cell.superview() }.is_some();
            shown || {
                away += 1;
                away <= KEPT_AWAY
            }
        });
    }
}

/// Reloads `outline` after its data source changed: when it still shows the same items in the same order, row by row
/// in place, which hands [`KeptCells`] their rows again; else whole.
pub fn reload(outline: &NSOutlineView) {
    let rows = usize::try_from(outline.numberOfRows()).unwrap_or(0);
    let same = shown_items(outline).is_some_and(|items| {
        items.len() == rows
            && items.iter().enumerate().all(|(row, item)| {
                outline
                    .itemAtRow(row.cast_signed())
                    .is_some_and(|shown| std::ptr::eq(&*shown, &**item))
                    && expandable_as_shown(outline, item)
            })
    });
    if !same || rows == 0 {
        outline.reloadData();
        return;
    }
    let columns = usize::try_from(outline.numberOfColumns()).unwrap_or(0);
    let rows = NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, rows));
    let columns = NSIndexSet::indexSetWithIndexesInRange(NSRange::new(0, columns));
    outline.reloadDataForRowIndexes_columnIndexes(&rows, &columns);
}

/// Whether the outline knows `item` as expandable as its data source says, which reloading rows in place leaves as it
/// was.
fn expandable_as_shown(outline: &NSOutlineView, item: &AnyObject) -> bool {
    // SAFETY: Asking for the data source only reads; the panel that set it keeps it alive.
    let Some(source) = (unsafe { outline.dataSource() }) else {
        return true;
    };
    // SAFETY: The data source method with its signature; `item` is one the data source handed out.
    let wanted: bool = unsafe { msg_send![&*source, outlineView: outline, isItemExpandable: item] };
    // SAFETY: As above.
    wanted == unsafe { outline.isExpandable(Some(item)) }
}

/// The items the data source of `outline` has now, in the order the outline would show them with the items it has
/// open, or `None` without a data source.
fn shown_items(outline: &NSOutlineView) -> Option<Vec<Retained<AnyObject>>> {
    // SAFETY: Asking for the data source only reads; the panel that set it keeps it alive.
    let source = unsafe { outline.dataSource() }?;
    let mut items = Vec::new();
    add_shown(outline, &source, None, &mut items);
    Some(items)
}

/// Adds the children of `parent` to `items` in the order the outline shows them, each followed by its own when open.
fn add_shown(
    outline: &NSOutlineView,
    source: &objc2::runtime::ProtocolObject<dyn objc2_app_kit::NSOutlineViewDataSource>,
    parent: Option<&AnyObject>,
    items: &mut Vec<Retained<AnyObject>>,
) {
    // SAFETY: The data source method with its signature; `parent` is nil or an item the data source handed out.
    let count: isize = unsafe { msg_send![source, outlineView: outline, numberOfChildrenOfItem: parent] };
    for index in 0..count {
        // SAFETY: As above, for an index below the count the data source gave.
        let child: Retained<AnyObject> =
            unsafe { msg_send![source, outlineView: outline, child: index, ofItem: parent] };
        // SAFETY: The item is one the data source handed out.
        let open = unsafe { outline.isItemExpanded(Some(&child)) };
        items.push(child.clone());
        if open {
            add_shown(outline, source, Some(&child), items);
        }
    }
}
