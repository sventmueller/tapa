//! A workspace (E20): a base folder with its tabs, one of them in front; and the ordered list with one item selected
//! that holds both a window's workspaces and a workspace's tabs.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use term_ui::workspace_host::{TabId, WorkspaceId};

use crate::tab::{Tab, TabContent};

/// What an ordered list finds its items by.
pub(crate) trait Keyed {
    /// The item's id, unique in the app.
    type Id: Copy + Eq;

    fn key(&self) -> Self::Id;
}

/// Items in the order the user sees them, at most one of them selected.
pub(crate) struct Ordered<T: Keyed> {
    items: Vec<Rc<T>>,
    selected: Option<T::Id>,
}

impl<T: Keyed> Ordered<T> {
    pub(crate) fn new() -> Ordered<T> {
        Ordered {
            items: Vec::new(),
            selected: None,
        }
    }

    pub(crate) fn items(&self) -> &[Rc<T>] {
        &self.items
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn get(&self, id: T::Id) -> Option<Rc<T>> {
        self.items.iter().find(|item| item.key() == id).cloned()
    }

    pub(crate) fn position(&self, id: T::Id) -> Option<usize> {
        self.items.iter().position(|item| item.key() == id)
    }

    pub(crate) fn selected_id(&self) -> Option<T::Id> {
        self.selected
    }

    pub(crate) fn selected(&self) -> Option<Rc<T>> {
        self.get(self.selected?)
    }

    /// Selects item `id`; whether that changed the selection.
    pub(crate) fn select(&mut self, id: T::Id) -> bool {
        if self.position(id).is_none() {
            return false;
        }
        self.selected.replace(id) != Some(id)
    }

    /// Adds `item` at the end.
    pub(crate) fn push(&mut self, item: Rc<T>) {
        self.items.push(item);
    }

    /// Adds `item` right after the selected one, at the end without a selection, as browsers open a new tab.
    pub(crate) fn insert_after_selected(&mut self, item: Rc<T>) {
        let at = self
            .selected
            .and_then(|id| self.position(id))
            .map_or(self.items.len(), |index| index + 1);
        self.items.insert(at, item);
    }

    /// Takes item `id` out; when it was selected, the one that took its place is, else the one before it.
    pub(crate) fn remove(&mut self, id: T::Id) -> Option<Rc<T>> {
        let at = self.position(id)?;
        let item = self.items.remove(at);
        if self.selected == Some(id) {
            self.selected = self
                .items
                .get(at.min(self.items.len().saturating_sub(1)))
                .map(|next| next.key());
        }
        Some(item)
    }

    /// Moves item `id` to `index`, or to the end past it.
    pub(crate) fn move_to(&mut self, id: T::Id, index: usize) {
        if let Some(at) = self.position(id) {
            let item = self.items.remove(at);
            self.items.insert(index.min(self.items.len()), item);
        }
    }

    /// Puts the items `ids` into that order at the places they hold among the others, as the user dragged them in a
    /// window's tab bar; unknown ids leave the order as it is.
    pub(crate) fn arrange(&mut self, ids: &[T::Id]) {
        let mut places: Vec<usize> = ids.iter().filter_map(|id| self.position(*id)).collect();
        if places.len() != ids.len() {
            return;
        }
        places.sort_unstable();
        let items: Vec<Rc<T>> = ids.iter().filter_map(|id| self.get(*id)).collect();
        for (place, item) in places.into_iter().zip(items) {
            self.items[place] = item;
        }
    }

    /// The id of the item `step` places from the selected one, going round at either end.
    pub(crate) fn step(&self, step: isize) -> Option<T::Id> {
        let at = isize::try_from(self.position(self.selected?)?).ok()?;
        let count = isize::try_from(self.items.len()).ok()?;
        let next = usize::try_from((at + step).rem_euclid(count)).ok()?;
        self.items.get(next).map(|item| item.key())
    }
}

/// A base folder with its tabs.
pub(crate) struct Workspace {
    pub(crate) id: u64,
    pub(crate) base: PathBuf,
    /// Whether it is the window's Terminal: always first, started anew with an empty terminal in the settings'
    /// folder, never stored and never removed; the others stay until the user removes them.
    pub(crate) scratch: bool,
    /// The name the user gave it; without one, the base folder's.
    name: RefCell<Option<String>>,
    branch: RefCell<Option<String>>,
    /// Whether the user trusts it, so that its agents load the project's hooks, MCP servers and settings (E21).
    trusted: Cell<bool>,
    pub(crate) tabs: RefCell<Ordered<Tab>>,
    /// The file list and the Git panel in its sidebar entry (E22).
    pub(crate) panels: RefCell<crate::panels::Panels>,
    /// How many files of its repository have changes, as its Git panel counted them last.
    changes: Cell<usize>,
}

impl Keyed for Workspace {
    type Id = WorkspaceId;

    fn key(&self) -> Self::Id {
        WorkspaceId::new(self.id)
    }
}

impl Keyed for Tab {
    type Id = TabId;

    fn key(&self) -> Self::Id {
        TabId::new(self.id)
    }
}

impl Workspace {
    /// A workspace without tabs in `base`, named `name` or after its folder.
    pub(crate) fn new(id: u64, base: PathBuf, name: Option<String>) -> Workspace {
        let branch = term_ui_sidebar::git::branch(&base);
        Workspace {
            id,
            scratch: false,
            base,
            name: RefCell::new(name.filter(|name| !name.trim().is_empty())),
            branch: RefCell::new(branch),
            trusted: Cell::new(false),
            tabs: RefCell::new(Ordered::new()),
            panels: RefCell::new(crate::panels::Panels::default()),
            changes: Cell::new(0),
        }
    }

    /// The window's Terminal in `base`.
    pub(crate) fn scratch(id: u64, base: PathBuf) -> Workspace {
        Workspace {
            scratch: true,
            ..Workspace::new(id, base, None)
        }
    }

    /// The name the sidebar shows.
    pub(crate) fn name(&self) -> String {
        if self.scratch {
            return splat_i18n::t("Terminal").to_string();
        }
        self.custom_name()
            .unwrap_or_else(|| term_ui_sidebar::model::folder_name(&self.base, &term_base::home()))
    }

    /// The name the user gave it, if any.
    pub(crate) fn custom_name(&self) -> Option<String> {
        self.name.borrow().clone()
    }

    /// Names it `name`; an empty name gives it its folder's again.
    pub(crate) fn rename(&self, name: &str) {
        if self.scratch {
            return;
        }
        let name = name.trim();
        *self.name.borrow_mut() = (!name.is_empty()).then(|| name.to_string());
    }

    pub(crate) fn is_trusted(&self) -> bool {
        self.trusted.get()
    }

    /// Trusts it or takes the trust back; its Git panel finds the repository again under the new policy (E16, E21).
    pub(crate) fn set_trusted(&self, trusted: bool) {
        if self.trusted.replace(trusted) != trusted {
            term_ui_git::trust_changed(&self.handle());
        }
    }

    /// The handle the UI crates know it by.
    pub(crate) fn handle(&self) -> term_ui::workspace_host::Workspace {
        term_ui::workspace_host::Workspace::new(term_ui::workspace_host::WorkspaceId::new(self.id))
    }

    /// Shows `count` changed files on the Git symbol of its row; whether that changed the number.
    pub(crate) fn set_changes(&self, count: usize) -> bool {
        self.changes.replace(count) != count
    }

    /// Reads the base folder's branch again, as a command may have switched it.
    pub(crate) fn refresh_branch(&self) {
        *self.branch.borrow_mut() = term_ui_sidebar::git::branch(&self.base);
    }

    /// Its tabs in their order.
    pub(crate) fn tabs(&self) -> Vec<Rc<Tab>> {
        self.tabs.borrow().items().to_vec()
    }

    /// The tab in front.
    pub(crate) fn select_tab(&self, id: TabId) {
        self.tabs.borrow_mut().select(id);
    }

    /// The selected tab.
    pub(crate) fn selected_tab(&self) -> Option<Rc<Tab>> {
        self.tabs.borrow().selected()
    }

    /// The terminal tab whose panes hold session view `view`.
    pub(crate) fn tab_of_view(&self, view: u64) -> Option<Rc<Tab>> {
        self.tabs.borrow().items().iter().find(|tab| tab.holds(view)).cloned()
    }

    /// The programs that run in its tabs, which closing it would end.
    pub(crate) fn busy(&self) -> Vec<String> {
        self.tabs().iter().flat_map(|tab| tab.busy()).collect()
    }

    /// Its row in the sidebar.
    pub(crate) fn entry(&self) -> term_ui_sidebar::Workspace {
        term_ui_sidebar::Workspace {
            id: self.id,
            name: self.name(),
            base: self.base.clone(),
            branch: self.branch.borrow().clone(),
            tabs: self.tabs().iter().map(|tab| tab.entry()).collect(),
            trusted: self.is_trusted(),
            fixed: self.scratch,
            panels: self.panels.borrow().open(),
            changes: self.changes.get(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Item(u64);

    impl Keyed for Item {
        type Id = u64;

        fn key(&self) -> u64 {
            self.0
        }
    }

    fn list(ids: &[u64], selected: u64) -> Ordered<Item> {
        let mut list = Ordered::new();
        for id in ids {
            list.push(Rc::new(Item(*id)));
        }
        list.select(selected);
        list
    }

    fn ids(list: &Ordered<Item>) -> Vec<u64> {
        list.items().iter().map(|item| item.0).collect()
    }

    #[test]
    fn a_new_item_goes_right_after_the_selected_one() {
        let mut tabs = list(&[1, 2, 3], 1);
        tabs.insert_after_selected(Rc::new(Item(4)));
        assert_eq!(ids(&tabs), [1, 4, 2, 3]);
        let mut empty: Ordered<Item> = Ordered::new();
        empty.insert_after_selected(Rc::new(Item(9)));
        assert_eq!(ids(&empty), [9]);
        assert_eq!(empty.selected_id(), None);
    }

    #[test]
    fn closing_the_selected_item_selects_its_neighbour() {
        let mut tabs = list(&[1, 2, 3], 2);
        tabs.remove(2);
        assert_eq!(tabs.selected_id(), Some(3), "the one that took its place");
        tabs.remove(3);
        assert_eq!(tabs.selected_id(), Some(1), "the one before the last");
        tabs.remove(1);
        assert_eq!(tabs.selected_id(), None);
        assert!(tabs.is_empty());
        let mut others = list(&[1, 2, 3], 1);
        others.remove(3);
        assert_eq!(others.selected_id(), Some(1), "closing another keeps the selection");
    }

    #[test]
    fn steps_go_round() {
        let tabs = list(&[1, 2, 3], 3);
        assert_eq!(tabs.step(1), Some(1));
        assert_eq!(tabs.step(-1), Some(2));
    }

    #[test]
    fn a_dragged_tab_takes_its_new_place() {
        let mut tabs = list(&[1, 2, 3, 4], 1);
        tabs.move_to(1, 2);
        assert_eq!(ids(&tabs), [2, 3, 1, 4]);
        tabs.move_to(4, 0);
        assert_eq!(ids(&tabs), [4, 2, 3, 1]);
        tabs.move_to(2, 99);
        assert_eq!(ids(&tabs), [4, 3, 1, 2]);
        assert_eq!(tabs.selected_id(), Some(1));
        assert!(!tabs.select(99), "an unknown item is not selected");
    }

    #[test]
    fn the_order_of_a_tab_bar_comes_back_at_the_places_its_tabs_hold() {
        let mut tabs = list(&[1, 2, 3, 4, 5], 1);
        tabs.arrange(&[4, 2]);
        assert_eq!(
            ids(&tabs),
            [1, 4, 3, 2, 5],
            "a tab dragged in one window leaves the others in place"
        );
        tabs.arrange(&[5, 1, 3]);
        assert_eq!(ids(&tabs), [5, 4, 1, 2, 3]);
        tabs.arrange(&[2, 9]);
        assert_eq!(ids(&tabs), [5, 4, 1, 2, 3], "an unknown tab changes nothing");
        assert_eq!(tabs.selected_id(), Some(1));
    }
}
