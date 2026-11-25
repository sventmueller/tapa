//! The self-test's hooks of `toolbar.rs`.

use super::*;

/// What the toolbar's branch item of `controller` shows: its title and its choices, `None` while it is hidden.
pub fn seen(controller: &(impl AsWorkspace + ?Sized)) -> Vec<Option<(String, Vec<String>)>> {
    let controller = &controller.as_workspace();
    let state = crate::host::state();
    let shown = state.toolbar_items.borrow();
    shown
        .iter()
        .filter(|s| s.controller == *controller)
        .filter_map(|s| Some((s.item.load()?, s.popup.load()?)))
        .map(|(item, popup)| {
            let hidden = if item.respondsToSelector(sel!(isHidden)) {
                item.isHidden()
            } else {
                !item.isEnabled()
            };
            (!hidden).then(|| {
                let titles: Vec<String> = popup.itemArray().iter().map(|i| i.title().to_string()).collect();
                (
                    titles.first().cloned().unwrap_or_default(),
                    titles.into_iter().skip(1).collect(),
                )
            })
        })
        .collect()
}

/// Chooses the entry `title` of the toolbar's branch item of `controller`, as a click on it does; whether it is there.
pub fn choose(controller: &(impl AsWorkspace + ?Sized), title: &str) -> bool {
    let controller = &controller.as_workspace();
    let popup = crate::host::state()
        .toolbar_items
        .borrow()
        .iter()
        .filter(|s| s.controller == *controller)
        .find_map(|s| s.popup.load());
    let Some(menu) = popup.and_then(|popup| popup.menu()) else {
        return false;
    };
    let found = menu
        .itemArray()
        .iter()
        .skip(1)
        .position(|item| item.title().to_string() == title)
        .map(|index| index + 1);
    if let Some(index) = found.and_then(|i| isize::try_from(i).ok()) {
        menu.performActionForItemAtIndex(index);
    }
    found.is_some()
}
