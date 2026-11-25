//! Moving a divider with the keyboard (F43): the split nearest the active pane that runs the right way gives a
//! twentieth of its room from one part to its neighbour.

use term_panes::{Node, PaneId, PaneTree, SplitId};

/// Which way the divider moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Left.
    Left,
    /// Right.
    Right,
    /// Up.
    Up,
    /// Down.
    Down,
}

/// How much of a split's room one step moves.
const STEP: f64 = 0.05;

/// The smallest share a part keeps.
const LEAST: f64 = 0.05;

/// The splits from the root down to `pane`, each with the index of the part that leads there.
fn path(node: &Node, pane: PaneId, into: &mut Vec<(SplitId, bool, usize, Vec<f64>)>) -> bool {
    match node {
        Node::Pane(id) => *id == pane,
        Node::Split(split) => {
            for (index, child) in split.children().iter().enumerate() {
                into.push((split.id(), split.side_by_side(), index, split.shares().to_vec()));
                if path(child, pane, into) {
                    return true;
                }
                into.pop();
            }
            false
        }
    }
}

/// Moves the divider beside the active pane of `tree` towards `direction`; whether anything moved.
pub(crate) fn step<D: Copy + Eq>(tree: &mut PaneTree<D>, direction: Direction) -> bool {
    let side_by_side = matches!(direction, Direction::Left | Direction::Right);
    let towards_start = matches!(direction, Direction::Left | Direction::Up);
    let mut splits = Vec::new();
    if !path(tree.root(), tree.active(), &mut splits) {
        return false;
    }
    let Some((id, _, index, mut shares)) = splits
        .into_iter()
        .rev()
        .find(|(_, runs, _, shares)| *runs == side_by_side && shares.len() > 1)
    else {
        return false;
    };
    let divider = if index + 1 < shares.len() { index } else { index - 1 };
    let (before, after) = (divider, divider + 1);
    let amount = if towards_start { -STEP } else { STEP };
    let moved = amount.max(LEAST - shares[before]).min(shares[after] - LEAST);
    if moved.abs() < f64::EPSILON {
        return false;
    }
    shares[before] += moved;
    shares[after] -= moved;
    tree.set_shares(id, &shares)
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_panes::{Place, Side};

    fn split_right() -> PaneTree<u64> {
        let mut tree = PaneTree::new();
        tree.insert(1, Place::Active);
        tree.insert(
            2,
            Place::Beside {
                anchor: 1,
                side: Side::Right,
                companion: false,
            },
        );
        tree
    }

    fn shares(tree: &PaneTree<u64>) -> Vec<f64> {
        tree.root().splits()[0].shares().to_vec()
    }

    #[test]
    fn the_divider_moves_left_and_right_but_not_up() {
        let mut tree = split_right();
        assert!(step(&mut tree, Direction::Left));
        let after = shares(&tree);
        assert!((after[0] - 0.45).abs() < 1e-9, "{after:?}");
        assert!(step(&mut tree, Direction::Right));
        assert!((shares(&tree)[0] - 0.5).abs() < 1e-9);
        assert!(!step(&mut tree, Direction::Up));
    }

    #[test]
    fn a_part_keeps_its_least_share() {
        let mut tree = split_right();
        for _ in 0..40 {
            step(&mut tree, Direction::Left);
        }
        assert!(shares(&tree)[0] >= LEAST - 1e-9);
    }
}
