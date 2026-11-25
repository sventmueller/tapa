//! A tab's panes made again from their stored shape (F71).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use term_panes::{PaneTree, Place, layout};

use super::{Inner, Panes};

impl Panes {
    /// The panes of a tab divided as `stored`, each stored session turned into a session view's id and view by
    /// `open`; none for a shape without a session.
    pub fn from_layout<'a, T>(
        mtm: MainThreadMarker,
        stored: &'a layout::Node<T>,
        mut open: impl FnMut(&'a T) -> (u64, Retained<NSView>),
    ) -> Option<Panes> {
        let mut tree = PaneTree::new();
        let mut views = HashMap::new();
        let built = tree.build(stored);
        for (pane, saved) in &built {
            for (index, document) in saved.documents.iter().enumerate() {
                let (id, view) = open(document);
                tree.insert(id, Place::Into(*pane, None));
                if index == saved.selected {
                    tree.select(id);
                }
                views.insert(id, view);
            }
        }
        tree.prune();
        if tree.is_empty() {
            return None;
        }
        if let Some((first, _)) = built.first() {
            tree.set_active(*first);
        }
        let container = NSView::new(mtm);
        container.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let panes = Panes(Rc::new(Inner {
            mtm,
            tree: RefCell::new(tree),
            container,
            views: RefCell::new(views),
            delegates: RefCell::new(Vec::new()),
            building: Cell::new(false),
        }));
        panes.rebuild();
        Some(panes)
    }
}
