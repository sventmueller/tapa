//! The tree as `layout` keeps it, and back: the parts with their shares and the documents of each pane with the one
//! in front.

use super::{Node, PaneId, PaneTree, Split};
use crate::layout;

impl<D: Copy + Eq> PaneTree<D> {
    /// The tree as the layout file keeps it, `reopen` saying how each document opens again; a preview's pane keeps
    /// nothing, as a preview opens with its document. None without a document that opens again.
    pub fn to_layout<T>(&self, reopen: &dyn Fn(D) -> Option<T>) -> Option<layout::Node<T>> {
        self.stored(&self.root, reopen).normalised()
    }

    fn stored<T>(&self, node: &Node, reopen: &dyn Fn(D) -> Option<T>) -> layout::Node<T> {
        match node {
            Node::Pane(id) => {
                let mut documents = Vec::new();
                let mut selected = 0;
                if let Some(pane) = self.pane(*id).filter(|pane| pane.companion_of.is_none()) {
                    let front = pane.front();
                    for document in &pane.documents {
                        if let Some(tab) = reopen(*document) {
                            if Some(*document) == front {
                                selected = documents.len();
                            }
                            documents.push(tab);
                        }
                    }
                }
                layout::Node::Pane(layout::Pane { documents, selected })
            }
            Node::Split(split) => layout::Node::Split {
                side_by_side: split.side_by_side,
                children: split.children.iter().map(|child| self.stored(child, reopen)).collect(),
                shares: split.shares.clone(),
            },
        }
    }

    /// Divides the tree, which holds one empty pane, as `layout` says: a lone pane keeps the one there is, a split
    /// gets new panes numbered from 1 in reading order, the first of them active. Each pane, empty, with the stored
    /// pane whose documents it is to show; none for a tree that holds more than one empty pane or a layout without a pane.
    pub fn build<'a, T>(&mut self, layout: &'a layout::Node<T>) -> Vec<(PaneId, &'a layout::Pane<T>)> {
        if self.panes.len() != 1 || !self.is_empty() || layout.panes().is_empty() {
            return Vec::new();
        }
        let mut panes = Vec::new();
        match layout {
            layout::Node::Pane(saved) => panes.push((self.active, saved)),
            layout::Node::Split { .. } => {
                self.panes.clear();
                self.next_pane = 1;
                self.root = self.build_node(layout, &mut panes);
                self.zoomed = None;
                if let Some((first, _)) = panes.first() {
                    self.active = *first;
                }
            }
        }
        panes
    }

    fn build_node<'a, T>(
        &mut self,
        layout: &'a layout::Node<T>,
        panes: &mut Vec<(PaneId, &'a layout::Pane<T>)>,
    ) -> Node {
        match layout {
            layout::Node::Pane(saved) => {
                let id = self.new_pane();
                panes.push((id, saved));
                Node::Pane(id)
            }
            layout::Node::Split {
                side_by_side,
                children,
                shares,
            } => {
                let mut split: Split = self.new_split(*side_by_side);
                for child in children.iter().filter(|child| !child.panes().is_empty()) {
                    let node = self.build_node(child, panes);
                    split.insert(split.children.len(), node, 0.5);
                }
                split.set_shares(shares);
                Node::Split(split)
            }
        }
    }
}
