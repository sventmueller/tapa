//! The editor's context menu: the text view's own entries, with the Format menu and "In Splat öffnen" in a Markdown
//! document.

use objc2::rc::Retained;
use objc2::{MainThreadOnly, sel};
use objc2_app_kit::{NSMenu, NSMenuItem};

use super::EditorTextView;
use crate::util::set_target;

/// Adds the Markdown entries to `menu`, the text view's context menu, after its cut, copy and paste.
pub(crate) fn extend(view: &EditorTextView, menu: &NSMenu) {
    if !crate::markdown::is_markdown(view) {
        return;
    }
    let mtm = view.mtm();
    let mut items: Vec<Retained<NSMenuItem>> = vec![crate::markdown::format_menu(mtm, &|_| String::new())];
    if let Some(path) = view.document().path() {
        items.extend(crate::markdown::open_in_splat_item(mtm, path));
    }
    let at = after_paste(menu);
    menu.insertItem_atIndex(&NSMenuItem::separatorItem(mtm), at);
    for (offset, item) in (1..).zip(&items) {
        aim(item, view);
        menu.insertItem_atIndex(item, at + offset);
    }
}

/// Sends the actions of `item` and its submenu to `view`, which need not be the first responder when it is clicked;
/// an item with a target of its own keeps it.
fn aim(item: &NSMenuItem, view: &EditorTextView) {
    match item.submenu() {
        Some(submenu) => {
            for child in submenu.itemArray().iter() {
                aim(&child, view);
            }
        }
        None if item.action().is_some() && item.target().is_none() => set_target(item, view),
        None => {}
    }
}

/// Where the group with Paste ends, or the menu's end.
fn after_paste(menu: &NSMenu) -> isize {
    let items = menu.itemArray();
    let paste = items.iter().position(|item| item.action() == Some(sel!(paste:)));
    let end = paste.and_then(|at| {
        items
            .iter()
            .skip(at)
            .position(|item| item.isSeparatorItem())
            .map(|offset| at + offset)
    });
    isize::try_from(end.unwrap_or(items.len())).unwrap_or(menu.numberOfItems())
}
