//! A tab's panes on screen (E12): one session per pane, split side by side or one above the other as deep as the user
//! splits, mirrored from `term_panes::PaneTree` into nested split views whenever its shape changes. Dividers write
//! their shares back into the tree; zoom shows the active pane alone; panes that do not have the focus are dimmed.
//! Written anew after Dango's `src/window/panes`, which is woven into its windows and documents (E2).

mod resize;
mod restore;
mod split;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use term_panes::layout;
use term_panes::{Node, PaneTree, Place};

pub use resize::Direction;
pub use term_panes::Side;

/// How strongly panes without the focus are dimmed, as their opacity.
const DIMMED: f64 = 0.72;

/// The panes of one tab, each holding the view of one session by the session view's id.
pub struct Panes(Rc<Inner>);

pub(crate) struct Inner {
    mtm: MainThreadMarker,
    pub(crate) tree: RefCell<PaneTree<u64>>,
    container: Retained<NSView>,
    views: RefCell<HashMap<u64, Retained<NSView>>>,
    delegates: RefCell<Vec<Retained<split::Delegate>>>,
    /// Set while the views are built, whose resizing is AppKit's, not the user's dragging a divider.
    pub(crate) building: std::cell::Cell<bool>,
}

impl Panes {
    /// The panes of a new tab holding the session `id`, whose view is `root`.
    pub fn new(mtm: MainThreadMarker, id: u64, root: Retained<NSView>) -> Panes {
        let container = NSView::new(mtm);
        container.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let mut tree = PaneTree::new();
        tree.insert(id, Place::Active);
        let inner = Rc::new(Inner {
            mtm,
            tree: RefCell::new(tree),
            container,
            views: RefCell::new(HashMap::from([(id, root)])),
            delegates: RefCell::new(Vec::new()),
            building: std::cell::Cell::new(false),
        });
        let panes = Panes(inner);
        panes.rebuild();
        panes
    }

    /// The view that shows the panes, which fills the tab.
    pub fn view(&self) -> &NSView {
        &self.0.container
    }

    /// The session of the active pane.
    pub fn focused(&self) -> Option<u64> {
        self.0.tree.borrow().focused()
    }

    /// The sessions, pane by pane in reading order.
    pub fn sessions(&self) -> Vec<u64> {
        self.0.tree.borrow().documents()
    }

    /// How many panes the tab has.
    pub fn len(&self) -> usize {
        self.sessions().len()
    }

    /// Whether the tab has no pane left.
    pub fn is_empty(&self) -> bool {
        self.0.tree.borrow().is_empty()
    }

    /// Whether the tab holds session `id`.
    pub fn holds(&self, id: u64) -> bool {
        self.0.tree.borrow().pane_of(id).is_some()
    }

    /// Whether one pane fills the tab for now.
    pub fn is_zoomed(&self) -> bool {
        self.0.tree.borrow().is_zoomed()
    }

    /// Puts session `id` with its view `root` into a new pane at `side` of the active one, which it becomes.
    pub fn split(&self, side: Side, id: u64, root: Retained<NSView>) {
        {
            let mut tree = self.0.tree.borrow_mut();
            if tree.is_zoomed() {
                let active = tree.active();
                tree.toggle_zoom(active);
            }
            let pane = match tree.focused() {
                Some(anchor) => tree.insert(
                    id,
                    Place::Beside {
                        anchor,
                        side,
                        companion: false,
                    },
                ),
                None => tree.insert(id, Place::Active),
            };
            tree.set_active(pane);
        }
        self.0.views.borrow_mut().insert(id, root);
        self.rebuild();
    }

    /// Takes session `id` out with its pane; whether the tab has none left.
    pub fn remove(&self, id: u64) -> bool {
        let removed = self.0.tree.borrow_mut().take(id);
        if let Some(view) = self.0.views.borrow_mut().remove(&id) {
            view.removeFromSuperview();
        }
        if removed && !self.is_empty() {
            self.rebuild();
        }
        self.is_empty()
    }

    /// Makes the pane of session `id` the active one, as clicking into it does.
    pub fn focus(&self, id: u64) {
        let changed = {
            let mut tree = self.0.tree.borrow_mut();
            match tree.pane_of(id) {
                Some(pane) if pane != tree.active() => {
                    tree.set_active(pane);
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.dim();
        }
    }

    /// Moves the focus `step` panes on, wrapping around; the session now active.
    pub fn step(&self, step: isize) -> Option<u64> {
        let stepped = self.0.tree.borrow_mut().step_pane(step).is_some();
        if stepped {
            self.rebuild();
        }
        self.focused()
    }

    /// Lets the active pane fill the tab, or shows all panes again.
    pub fn toggle_zoom(&self) {
        {
            let mut tree = self.0.tree.borrow_mut();
            let active = tree.active();
            tree.toggle_zoom(active);
        }
        self.rebuild();
    }

    /// Gives every part of every split the same share.
    pub fn equalize(&self) {
        {
            let mut tree = self.0.tree.borrow_mut();
            let splits: Vec<(term_panes::SplitId, usize)> = tree
                .root()
                .splits()
                .iter()
                .map(|split| (split.id(), split.children().len()))
                .collect();
            for (id, parts) in splits {
                #[expect(clippy::cast_precision_loss, reason = "a split has a handful of parts")]
                let share = 1.0 / parts.max(1) as f64;
                tree.set_shares(id, &vec![share; parts]);
            }
        }
        self.rebuild();
    }

    /// Moves the divider next to the active pane towards `direction` by a twentieth of the split's room (F43).
    pub fn resize(&self, direction: Direction) {
        if resize::step(&mut self.0.tree.borrow_mut(), direction) {
            self.rebuild();
        }
    }

    /// The tab's shape for storing, each pane's session turned into what reopens it by `reopen`.
    pub fn to_layout<T>(&self, reopen: &dyn Fn(u64) -> Option<T>) -> Option<layout::Node<T>> {
        self.0.tree.borrow().to_layout(reopen)
    }

    /// Builds the views anew from the tree: nested split views, or the zoomed pane alone.
    fn rebuild(&self) {
        let inner = &self.0;
        inner.building.set(true);
        self.rebuild_views();
        inner.building.set(false);
    }

    fn rebuild_views(&self) {
        let inner = &self.0;
        for child in inner.container.subviews().iter() {
            child.removeFromSuperview();
        }
        inner.delegates.borrow_mut().clear();
        let tree = inner.tree.borrow();
        let shown = if tree.is_zoomed() {
            tree.focused().and_then(|id| inner.views.borrow().get(&id).cloned())
        } else {
            drop(tree);
            self.build(&self.0.tree.borrow().root().clone())
        };
        let Some(shown) = shown else { return };
        shown.setFrame(inner.container.bounds());
        shown.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        inner.container.addSubview(&shown);
        split::apply_shares(&shown);
        self.dim();
    }

    fn build(&self, node: &Node) -> Option<Retained<NSView>> {
        match node {
            Node::Pane(pane) => {
                let id = self.0.tree.borrow().pane(*pane)?.front()?;
                self.0.views.borrow().get(&id).cloned()
            }
            Node::Split(split) => {
                let children: Vec<Retained<NSView>> =
                    split.children().iter().filter_map(|child| self.build(child)).collect();
                let (view, delegate) = split::make(
                    self.0.mtm,
                    split.id(),
                    split.side_by_side(),
                    split.shares(),
                    &children,
                    &self.0,
                );
                self.0.delegates.borrow_mut().push(delegate);
                Some(view)
            }
        }
    }

    /// Dims every pane but the active one while the tab is split.
    fn dim(&self) {
        let tree = self.0.tree.borrow();
        let focused = tree.focused();
        let split = tree.is_split() && !tree.is_zoomed();
        for (id, view) in self.0.views.borrow().iter() {
            let alpha = if !split || Some(*id) == focused { 1.0 } else { DIMMED };
            view.setAlphaValue(alpha);
        }
    }
}
