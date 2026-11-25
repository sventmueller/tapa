//! How a tab's panes are stored, with `T` saying what reopens a pane's session: parts side by side or one above the
//! other with their shares, and each pane's documents with the one in front.

use serde::{Deserialize, Serialize};

/// Documents behind one another in a part of a tab, one of them in front; in the terminal one session per pane (E12).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane<T> {
    /// The documents.
    pub documents: Vec<T>,
    /// The index of the document in front.
    #[serde(default)]
    pub selected: usize,
}

/// How a tab is divided: a pane, or parts side by side or one above the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Node<T> {
    /// One pane.
    Pane(Pane<T>),
    /// Parts next to one another.
    Split {
        /// Left to right when true, top to bottom when false.
        side_by_side: bool,
        /// The parts.
        children: Vec<Node<T>>,
        /// Each part's share of the width or height, adding up to 1.
        shares: Vec<f64>,
    },
}

impl<T> Node<T> {
    /// The panes, left to right and top to bottom.
    pub fn panes(&self) -> Vec<&Pane<T>> {
        fn walk<'a, T>(node: &'a Node<T>, into: &mut Vec<&'a Pane<T>>) {
            match node {
                Node::Pane(pane) => into.push(pane),
                Node::Split { children, .. } => children.iter().for_each(|child| walk(child, into)),
            }
        }
        let mut panes = Vec::new();
        walk(self, &mut panes);
        panes
    }

    /// The node without empty panes, without splits of fewer than two parts, with a split inside a split of the same
    /// direction merged into it, and with shares that add up to 1; `None` when no document is left.
    pub fn normalised(self) -> Option<Node<T>> {
        match self {
            Node::Pane(mut pane) => {
                if pane.documents.is_empty() {
                    return None;
                }
                pane.selected = pane.selected.min(pane.documents.len() - 1);
                Some(Node::Pane(pane))
            }
            Node::Split {
                side_by_side,
                children,
                shares,
            } => normalised_split(side_by_side, children, shares),
        }
    }
}

#[expect(clippy::cast_precision_loss, reason = "a split has a handful of parts")]
fn normalised_split<T>(side_by_side: bool, children: Vec<Node<T>>, shares: Vec<f64>) -> Option<Node<T>> {
    let even = 1.0 / children.len().max(1) as f64;
    let usable = shares.len() == children.len() && shares.iter().all(|share| share.is_finite() && *share > 0.0);
    let mut parts: Vec<(Node<T>, f64)> = Vec::new();
    for (index, child) in children.into_iter().enumerate() {
        let share = if usable { shares[index] } else { even };
        match child.normalised() {
            Some(Node::Split {
                side_by_side: inner,
                children,
                shares,
            }) if inner == side_by_side => {
                parts.extend(
                    children
                        .into_iter()
                        .zip(shares)
                        .map(|(node, part)| (node, part * share)),
                );
            }
            Some(node) => parts.push((node, share)),
            None => {}
        }
    }
    if parts.len() < 2 {
        return parts.pop().map(|(node, _)| node);
    }
    let total: f64 = parts.iter().map(|(_, share)| share).sum();
    let (children, shares) = parts.into_iter().map(|(node, share)| (node, share / total)).unzip();
    Some(Node::Split {
        side_by_side,
        children,
        shares,
    })
}

/// A tab and how its sessions are divided into panes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowTab<T> {
    /// The panes.
    pub root: Node<T>,
}

/// The tabs of a window, in the order of its tab bar, and the one in front.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout<T> {
    /// The tabs.
    pub tabs: Vec<WindowTab<T>>,
    /// The index of the tab in front.
    pub selected: Option<usize>,
}
