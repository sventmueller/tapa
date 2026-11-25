use serde_json::json;

use super::*;
use crate::layout;

type Tab = serde_json::Value;
type Layout = layout::Layout<Tab>;

type Tree = PaneTree<&'static str>;

fn pane(number: usize) -> PaneId {
    PaneId::from_number(number)
}

/// The tree in a line, as the window self-test writes it: a pane as its documents in brackets, the one in front
/// starred while there are more, the active pane with `!`; a split as `H(…)` side by side or `V(…)` one above the
/// other.
fn shape(tree: &Tree) -> String {
    fn of(tree: &Tree, node: &Node) -> String {
        match node {
            Node::Pane(id) => {
                let pane = tree.pane(*id).expect("a pane of the tree is in its list");
                let titles: Vec<String> = pane
                    .documents()
                    .iter()
                    .map(|doc| {
                        let star = if Some(*doc) == pane.front() && pane.count() > 1 {
                            "*"
                        } else {
                            ""
                        };
                        format!("{doc}{star}")
                    })
                    .collect();
                let mark = if *id == tree.active() { "!" } else { "" };
                format!("[{}]{mark}", titles.join(", "))
            }
            Node::Split(split) => {
                let kind = if split.side_by_side() { "H" } else { "V" };
                let children: Vec<String> = split.children().iter().map(|child| of(tree, child)).collect();
                format!("{kind}({})", children.join(" "))
            }
        }
    }
    of(tree, tree.root())
}

fn root_shares(tree: &Tree) -> Vec<f64> {
    match tree.root() {
        Node::Split(split) => split.shares().to_vec(),
        Node::Pane(_) => vec![1.0],
    }
}

fn root_split(tree: &Tree) -> SplitId {
    match tree.root() {
        Node::Split(split) => split.id(),
        Node::Pane(_) => panic!("the root is a split: {}", shape(tree)),
    }
}

fn with(documents: &[&'static str]) -> Tree {
    let mut tree = Tree::new();
    for document in documents {
        tree.insert(document, Place::Active);
    }
    tree
}

/// Three panes side by side holding `a`, `b` and `c`, the first active, with shares of a half and two quarters.
fn three_side_by_side() -> Tree {
    let mut tree = with(&["a"]);
    let second = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("b", Place::Into(second, None));
    let third = tree.add_beside(second, Side::Right).unwrap();
    tree.insert("c", Place::Into(third, None));
    tree
}

#[test]
fn a_new_tree_is_one_empty_active_pane() {
    let tree = Tree::new();
    assert_eq!(tree.all(), [pane(1)]);
    assert_eq!(tree.active(), pane(1));
    assert_eq!(tree.main(), pane(1));
    assert!(tree.is_empty() && !tree.is_split() && !tree.is_zoomed());
    assert_eq!(tree.focused(), None);
    assert_eq!(shape(&tree), "[]!");
}

#[test]
fn documents_open_in_the_active_pane_after_the_one_in_front() {
    let mut tree = with(&["a", "b"]);
    assert_eq!(shape(&tree), "[a, b*]!");
    assert!(tree.select("a"));
    tree.insert("c", Place::Active);
    assert_eq!(shape(&tree), "[a, c*, b]!");
    tree.insert("d", Place::Into(pane(1), Some(0)));
    assert_eq!(shape(&tree), "[d*, a, c, b]!");
    tree.insert("e", Place::Into(pane(1), Some(99)));
    assert_eq!(tree.documents(), ["d", "a", "c", "b", "e"]);
    assert_eq!(tree.focused(), Some("e"));
    assert_eq!(tree.pane_of("c"), Some(pane(1)));
    assert_eq!(tree.pane_of("z"), None);
}

#[test]
fn a_document_opens_at_most_once_and_comes_to_the_front_where_it_is() {
    let mut tree = with(&["a", "b"]);
    let (second, _) = tree.split_active(Side::Right).unwrap();
    tree.insert("c", Place::Into(second, None));
    assert_eq!(shape(&tree), "H([a] [b, c*]!)");
    assert_eq!(tree.insert("a", Place::Into(second, None)), pane(1));
    assert_eq!(shape(&tree), "H([a]! [b, c*])");
    assert_eq!(tree.insert("b", Place::Active), second);
    assert_eq!(shape(&tree), "H([a] [b*, c]!)");
    assert_eq!(tree.documents(), ["a", "b", "c"]);
}

#[test]
fn splitting_moves_the_document_in_front_into_the_new_pane_while_others_stay() {
    let mut tree = with(&["a", "b", "c"]);
    assert_eq!(tree.split_active(Side::Right), Some((pane(2), Some("c"))));
    assert_eq!(shape(&tree), "H([a, b*] [c]!)");
    assert!(tree.is_split());
    assert_eq!(root_shares(&tree), [0.5, 0.5]);
}

#[test]
fn splitting_a_pane_of_one_document_opens_an_empty_active_pane_for_the_next_document() {
    let mut tree = with(&["a"]);
    assert_eq!(tree.split_active(Side::Below), Some((pane(2), None)));
    assert_eq!(shape(&tree), "V([a] []!)");
    assert!(tree.is_split());
    tree.insert("b", Place::Active);
    assert_eq!(shape(&tree), "V([a] [b]!)");
    let mut empty = Tree::new();
    assert_eq!(empty.split_active(Side::Right), Some((pane(2), None)));
    assert_eq!(shape(&empty), "H([] []!)");
}

#[test]
fn a_split_the_same_way_joins_its_split_and_one_the_other_way_nests() {
    let mut tree = with(&["a", "b", "c"]);
    tree.split_active(Side::Right);
    tree.insert("d", Place::Active);
    assert_eq!(tree.split_active(Side::Below), Some((pane(3), Some("d"))));
    assert_eq!(shape(&tree), "H([a, b*] V([c] [d]!))");
    assert_eq!(tree.split_active(Side::Below), Some((pane(4), None)));
    assert_eq!(shape(&tree), "H([a, b*] V([c] [d] []!))");
    let Node::Split(root) = tree.root() else {
        panic!("{}", shape(&tree))
    };
    let Node::Split(column) = &root.children()[1] else {
        panic!("{}", shape(&tree))
    };
    assert_eq!(root.shares(), [0.5, 0.5]);
    assert_eq!(
        column.shares(),
        [0.5, 0.25, 0.25],
        "the new pane takes half of the room of its neighbour"
    );
    assert_eq!(tree.split_active(Side::Right), Some((pane(5), None)));
    assert_eq!(shape(&tree), "H([a, b*] V([c] [d] H([] []!)))");
    assert_eq!(tree.all(), [pane(1), pane(2), pane(3), pane(4), pane(5)]);
    assert_eq!(tree.root().splits().len(), 3);
}

#[test]
fn closing_the_active_pane_activates_the_last_pane_before_it_and_its_share_goes_there() {
    let mut tree = three_side_by_side();
    assert_eq!(shape(&tree), "H([a]! [b] [c])");
    assert_eq!(root_shares(&tree), [0.5, 0.25, 0.25]);
    tree.set_active(pane(2));
    assert!(tree.take("b"));
    assert_eq!(shape(&tree), "H([a]! [c])");
    assert_eq!(root_shares(&tree), [0.75, 0.25]);
    assert!(tree.take("a"));
    assert_eq!(
        shape(&tree),
        "[c]!",
        "the first pane closing gives the one after it the room and the focus"
    );
    assert!(!tree.is_split());
}

#[test]
fn closing_a_pane_that_is_not_active_keeps_the_active_one() {
    let mut tree = three_side_by_side();
    tree.set_active(pane(3));
    assert!(tree.take("a"));
    assert_eq!(shape(&tree), "H([b] [c]!)");
    assert_eq!(
        root_shares(&tree),
        [0.75, 0.25],
        "the share of the first goes to the one after it"
    );
}

#[test]
fn a_split_left_with_one_part_merges_into_the_split_around_it_with_its_share() {
    let mut tree = with(&["a"]);
    let second = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("b", Place::Into(second, None));
    let third = tree.add_beside(second, Side::Below).unwrap();
    tree.insert("c", Place::Into(third, None));
    let fourth = tree.add_beside(third, Side::Right).unwrap();
    tree.insert("d", Place::Into(fourth, None));
    assert_eq!(shape(&tree), "H([a]! V([b] H([c] [d])))");
    assert!(
        tree.set_shares(root_split(&tree), &[3.0, 1.0]),
        "shares are scaled to add up to 1"
    );
    assert_eq!(root_shares(&tree), [0.75, 0.25]);
    tree.set_active(second);
    assert!(tree.take("b"));
    assert_eq!(shape(&tree), "H([a] [c] [d]!)");
    assert_eq!(root_shares(&tree), [0.75, 0.125, 0.125]);
    assert_eq!(tree.root().splits().len(), 1);
}

#[test]
fn a_split_left_with_one_part_of_the_other_direction_takes_its_place() {
    let mut tree = with(&["a"]);
    let second = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("b", Place::Into(second, None));
    let third = tree.add_beside(second, Side::Below).unwrap();
    tree.insert("c", Place::Into(third, None));
    let fourth = tree.add_beside(third, Side::Below).unwrap();
    tree.insert("d", Place::Into(fourth, None));
    assert_eq!(shape(&tree), "H([a]! V([b] [c] [d]))");
    assert!(tree.take("a"));
    assert_eq!(
        shape(&tree),
        "V([b] [c] [d]!)",
        "the column fills the tab, its last pane active"
    );
    let Node::Split(column) = tree.root() else {
        panic!("{}", shape(&tree))
    };
    assert_eq!(column.shares(), [0.5, 0.25, 0.25]);
}

#[test]
fn the_last_pane_and_a_pane_with_documents_stay() {
    let mut tree = with(&["a"]);
    assert!(!tree.remove_pane(pane(1)));
    assert!(tree.take("a"));
    assert_eq!(shape(&tree), "[]!", "the last document closing leaves the pane");
    let mut tree = with(&["a"]);
    let (second, _) = tree.split_active(Side::Right).unwrap();
    assert!(!tree.remove_pane(pane(1)));
    assert!(!tree.remove_pane(pane(9)));
    assert!(tree.remove_pane(second));
    assert_eq!(shape(&tree), "[a]!");
}

#[test]
fn a_closing_pane_hands_its_documents_to_the_pane_before_else_after() {
    let mut tree = three_side_by_side();
    tree.insert("d", Place::Into(pane(3), None));
    tree.select("c");
    assert_eq!(tree.close_pane(pane(3)), Some(pane(2)));
    assert_eq!(
        shape(&tree),
        "H([a] [b*, c, d]!)",
        "the pane before keeps its document in front and takes the others behind it"
    );
    assert_eq!(tree.close_pane(pane(1)), Some(pane(2)));
    assert_eq!(
        shape(&tree),
        "[b*, c, d, a]!",
        "the first pane hands its documents to the one after"
    );
    assert!(!tree.is_split());
    assert_eq!(
        tree.close_pane(pane(2)),
        None,
        "the last pane has no pane to hand them to"
    );
}

#[test]
fn a_pane_closing_into_an_empty_one_brings_its_document_in_front_along() {
    let mut tree = with(&["a", "b"]);
    tree.split_active(Side::Right);
    let empty = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("c", Place::Into(pane(2), None));
    tree.select("b");
    assert_eq!(shape(&tree), "H([a] [] [b*, c]!)");
    assert_eq!(tree.close_pane(pane(2)), Some(empty));
    assert_eq!(shape(&tree), "H([a] [b*, c]!)");
}

#[test]
fn a_preview_pane_closes_with_its_preview_and_is_never_the_heir() {
    let mut tree = with(&["a", "b"]);
    let preview = tree.insert(
        "p",
        Place::Beside {
            anchor: "a",
            side: Side::Right,
            companion: true,
        },
    );
    assert_eq!(
        tree.close_pane(preview),
        None,
        "a preview follows its document and goes with it"
    );
    tree.set_active(pane(1));
    let (other, _) = tree.split_active(Side::Right).unwrap();
    assert_eq!(
        tree.close_pane(other),
        Some(pane(1)),
        "the documents skip the preview's pane"
    );
    assert_eq!(tree.pane(preview).map(Pane::count), Some(1));
}

#[test]
fn the_last_pane_but_one_closing_spreads_the_documents_onto_native_tabs_again() {
    let mut tree = with(&["a", "b", "c"]);
    tree.split_active(Side::Right);
    assert_eq!(tree.spread(), None, "a split tree keeps its documents in its panes");
    assert!(tree.take("c"));
    assert_eq!(shape(&tree), "[a, b*]!");
    assert!(!tree.is_split());
    assert_eq!(
        tree.spread(),
        Some(Spread {
            front: "b",
            before: vec![("a", None)],
            after: Vec::new(),
        })
    );
    tree.select("a");
    tree.insert(
        "p",
        Place::Beside {
            anchor: "a",
            side: Side::Right,
            companion: true,
        },
    );
    tree.insert("x", Place::Into(pane(1), Some(2)));
    tree.select("b");
    assert!(!tree.is_split(), "a preview beside its document is no split");
    assert_eq!(
        tree.spread(),
        Some(Spread {
            front: "b",
            before: vec![("a", Some("p"))],
            after: vec![("x", None)],
        })
    );
    assert_eq!(with(&["a"]).spread(), None, "one document keeps its tab");
}

#[test]
fn a_preview_follows_its_document_and_never_takes_another() {
    let mut tree = with(&["a", "b"]);
    tree.select("a");
    let preview = tree.insert(
        "p",
        Place::Beside {
            anchor: "a",
            side: Side::Right,
            companion: true,
        },
    );
    assert_eq!(tree.companion_of("a"), Some(preview));
    assert_eq!(tree.pane(preview).and_then(Pane::companion_of), Some("a"));
    assert_eq!(tree.main(), pane(1));
    assert!(tree.is_shown(preview));
    tree.select("b");
    assert!(
        !tree.is_shown(preview),
        "a preview hides while its document is behind another"
    );
    tree.set_active(preview);
    assert_eq!(tree.insert("c", Place::Active), pane(1));
    assert_eq!(shape(&tree), "H([a, b, c*] [p]!)");
    assert!(tree.take("p"));
    assert_eq!(shape(&tree), "[a, b, c*]!");
}

#[test]
fn a_document_beside_one_the_tree_does_not_hold_goes_into_the_active_pane() {
    let mut tree = with(&["a"]);
    let at = tree.insert(
        "b",
        Place::Beside {
            anchor: "z",
            side: Side::Right,
            companion: false,
        },
    );
    assert_eq!(at, pane(1));
    assert_eq!(shape(&tree), "[a, b*]!");
    assert_eq!(tree.insert("c", Place::Into(pane(7), None)), pane(1));
}

#[test]
fn a_zoomed_pane_shows_alone_until_zoomed_again_or_split() {
    let mut tree = three_side_by_side();
    tree.toggle_zoom(pane(2));
    assert!(tree.is_zoomed());
    assert_eq!(
        tree.all().into_iter().map(|id| tree.is_shown(id)).collect::<Vec<_>>(),
        [false, true, false]
    );
    tree.toggle_zoom(pane(2));
    assert!(!tree.is_zoomed());
    assert!(tree.all().into_iter().all(|id| tree.is_shown(id)));
    tree.toggle_zoom(pane(1));
    tree.add_beside(pane(3), Side::Below);
    assert!(!tree.is_zoomed(), "splitting shows every pane again");
    tree.toggle_zoom(pane(2));
    tree.set_active(pane(2));
    assert!(tree.take("b"));
    assert!(!tree.is_zoomed(), "the zoomed pane closing shows the others");
    tree.toggle_zoom(pane(42));
    assert!(!tree.is_zoomed());
}

#[test]
fn the_next_and_previous_pane_wrap_around_and_show_every_pane() {
    let mut tree = three_side_by_side();
    assert_eq!(tree.step_pane(1), Some(pane(2)));
    assert_eq!(tree.step_pane(1), Some(pane(3)));
    assert_eq!(tree.step_pane(1), Some(pane(1)));
    assert_eq!(tree.step_pane(-1), Some(pane(3)));
    assert_eq!(tree.active(), pane(3));
    tree.toggle_zoom(pane(3));
    assert_eq!(tree.step_pane(1), Some(pane(1)));
    assert!(!tree.is_zoomed());
    assert_eq!(with(&["a", "b"]).step_pane(1), None, "one pane has no other");
}

#[test]
fn moving_through_panes_leaves_out_a_preview_behind_its_document() {
    let mut tree = with(&["a", "b"]);
    tree.insert(
        "p",
        Place::Beside {
            anchor: "a",
            side: Side::Right,
            companion: true,
        },
    );
    tree.set_active(pane(1));
    assert_eq!(
        tree.step_pane(1),
        Some(pane(1)),
        "the preview of a document behind another is hidden"
    );
    tree.select("a");
    assert_eq!(tree.step_pane(1), Some(pane(2)));
}

#[test]
fn the_next_and_previous_tab_of_a_pane_wrap_around() {
    let mut tree = with(&["a", "b", "c"]);
    assert_eq!(tree.select_relative(1), Some("a"));
    assert_eq!(tree.select_relative(-1), Some("c"));
    assert_eq!(tree.select_relative(-1), Some("b"));
    assert_eq!(tree.select_relative(4), Some("c"));
    assert_eq!(with(&["a"]).select_relative(1), None);
}

#[test]
fn taking_a_document_brings_the_next_to_the_front_else_the_one_before() {
    let mut tree = with(&["a", "b", "c"]);
    tree.select("b");
    assert!(tree.take("b"));
    assert_eq!(tree.focused(), Some("c"));
    assert!(tree.take("c"));
    assert_eq!(tree.focused(), Some("a"));
    tree.insert("d", Place::Into(pane(1), Some(0)));
    tree.select("a");
    assert!(tree.take("d"));
    assert_eq!(tree.focused(), Some("a"), "one before the front keeps the front");
    assert!(!tree.take("z"));
}

#[test]
fn a_document_moved_into_another_pane_leaves_no_empty_pane_behind() {
    let mut tree = with(&["a"]);
    let second = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("b", Place::Into(second, None));
    assert!(tree.move_document("b", pane(1), None));
    assert_eq!(shape(&tree), "[a, b*]!");
    assert!(!tree.move_document("b", pane(9), None));
    assert!(!tree.move_document("z", pane(1), None));
    let mut row = with(&["a", "b", "c"]);
    assert!(
        row.move_document("a", pane(1), Some(2)),
        "along its own bar a document only moves"
    );
    assert_eq!(shape(&row), "[b, c, a*]!");
}

#[test]
fn a_document_splits_off_only_from_a_pane_that_keeps_another() {
    let mut tree = with(&["a"]);
    assert_eq!(tree.split_off("a", Side::Right), None);
    tree.insert("b", Place::Active);
    assert_eq!(tree.split_off("a", Side::Below), Some(pane(2)));
    assert_eq!(shape(&tree), "V([b] [a]!)");
}

#[test]
fn a_dragged_tab_lands_by_the_part_of_the_pane_it_is_over() {
    let mut tree = with(&["a"]);
    let second = tree.add_beside(pane(1), Side::Right).unwrap();
    tree.insert("b", Place::Into(second, None));
    tree.insert("c", Place::Into(second, None));
    let edge = |side| Some(Hit::Content(second, Some(side)));
    assert_eq!(
        tree.landing("a", edge(Side::Below)),
        Landing::Beside(second, Side::Below)
    );
    assert_eq!(
        tree.landing("a", Some(Hit::Content(pane(1), Some(Side::Right)))),
        Landing::Nowhere,
        "a document alone in its pane does not split it off"
    );
    assert_eq!(
        tree.landing("b", edge(Side::Right)),
        Landing::Beside(second, Side::Right)
    );
    assert_eq!(
        tree.landing("a", Some(Hit::Content(second, None))),
        Landing::Into(second, None)
    );
    assert_eq!(tree.landing("b", Some(Hit::Content(second, None))), Landing::Nowhere);
    assert_eq!(
        tree.landing("a", Some(Hit::Bar(second, 0))),
        Landing::Into(second, Some(0))
    );
    assert_eq!(tree.landing("a", None), Landing::Nowhere);
    assert_eq!(tree.landing("z", Some(Hit::Bar(second, 0))), Landing::Nowhere);
    assert!(tree.drop_at("b", Landing::Beside(second, Side::Right)));
    assert_eq!(shape(&tree), "H([a] [c] [b]!)");
    assert!(tree.drop_at("c", Landing::Into(pane(1), Some(0))));
    assert_eq!(shape(&tree), "H([c*, a]! [b])");
    assert!(!tree.drop_at("c", Landing::Nowhere));
}

#[test]
fn shares_that_do_not_fit_are_refused() {
    let mut tree = three_side_by_side();
    let id = root_split(&tree);
    assert!(!tree.set_shares(id, &[0.5, 0.5]));
    assert!(!tree.set_shares(id, &[0.5, 0.0, 0.5]));
    assert!(!tree.set_shares(id, &[0.5, f64::NAN, 0.5]));
    assert!(!tree.set_shares(SplitId(99), &[1.0]));
    assert_eq!(root_shares(&tree), [0.5, 0.25, 0.25]);
    assert!(tree.set_shares(id, &[1.0, 1.0, 2.0]));
    assert_eq!(root_shares(&tree), [0.25, 0.25, 0.5]);
    assert_eq!(tree.split(id).map(Split::side_by_side), Some(true));
}

/// A stored layout as Dango's layout file of version 4 keeps it: a window tab split side by side whose right part is split
/// one above the other, and a window tab with one document.
const STORED: &str = r#"{
  "tabs": {
    "selected": 0,
    "tabs": [
      {
        "root": {
          "split": {
            "children": [
              {
                "pane": {
                  "documents": [
                    {
                      "kind": "editor",
                      "path": "/p/src/main.rs"
                    },
                    {
                      "kind": "welcome"
                    }
                  ],
                  "selected": 1
                }
              },
              {
                "split": {
                  "children": [
                    {
                      "pane": {
                        "documents": [
                          {
                            "kind": "browser",
                            "url": "http://localhost:5173"
                          }
                        ],
                        "selected": 0
                      }
                    },
                    {
                      "pane": {
                        "documents": [
                          {
                            "folder": "/q",
                            "kind": "terminal"
                          },
                          {
                            "kind": "agent",
                            "session": "abc"
                          }
                        ],
                        "selected": 0
                      }
                    }
                  ],
                  "shares": [
                    0.25,
                    0.75
                  ],
                  "side_by_side": false
                }
              }
            ],
            "shares": [
              0.75,
              0.25
            ],
            "side_by_side": true
          }
        }
      },
      {
        "root": {
          "pane": {
            "documents": [
              {
                "kind": "terminal",
                "folder": "/p"
              }
            ],
            "selected": 0
          }
        }
      }
    ]
  },
  "version": 4
}
"#;

/// Opens the documents of `root` into a new tree as the window does: each pane built, each document after the one
/// before, the stored one brought to the front; the documents get their index in `opened`.
fn restore(root: &layout::Node<Tab>, opened: &mut Vec<Tab>) -> PaneTree<usize> {
    let mut tree = PaneTree::new();
    let built = tree.build(root);
    let mut fronts = Vec::new();
    for (id, saved) in &built {
        for (index, tab) in saved.documents.iter().enumerate() {
            opened.push(tab.clone());
            tree.insert(opened.len() - 1, Place::Into(*id, None));
            if index == saved.selected {
                fronts.push(opened.len() - 1);
            }
        }
    }
    for front in fronts {
        tree.select(front);
    }
    tree.prune();
    tree
}

#[test]
fn a_stored_layout_comes_back_as_it_was_stored() {
    let file: serde_json::Value = serde_json::from_str(STORED).unwrap();
    assert_eq!(file["version"], 4);
    let stored: Layout = serde_json::from_value(file["tabs"].clone()).unwrap();
    let mut written = Layout {
        tabs: Vec::new(),
        selected: stored.selected,
    };
    for window_tab in &stored.tabs {
        let mut opened = Vec::new();
        let tree = restore(&window_tab.root, &mut opened);
        let root = tree
            .to_layout(&|id| opened.get(id).cloned())
            .expect("the documents open again");
        written.tabs.push(layout::WindowTab { root });
    }
    assert_eq!(written, stored);
    assert_eq!(serde_json::to_value(&written).unwrap(), file["tabs"]);
}

#[test]
fn a_restored_split_has_its_shapes_shares_and_fronts() {
    let file: serde_json::Value = serde_json::from_str(STORED).unwrap();
    let stored: Layout = serde_json::from_value(file["tabs"].clone()).unwrap();
    let mut opened = Vec::new();
    let tree = restore(&stored.tabs[0].root, &mut opened);
    let panes = tree.all();
    assert_eq!(panes.len(), 3);
    assert_eq!(
        tree.active(),
        panes[0],
        "the first pane of a restored split is the active one"
    );
    assert!(tree.is_split());
    let fronts: Vec<Option<Tab>> = panes
        .iter()
        .map(|id| tree.pane(*id).and_then(Pane::front).map(|doc| opened[doc].clone()))
        .collect();
    assert_eq!(
        fronts,
        [
            Some(document(json!({"kind": "welcome"}))),
            Some(document(json!({"kind": "browser", "url": "http://localhost:5173"}))),
            Some(document(json!({"kind": "terminal", "folder": "/q"}))),
        ]
    );
    let Node::Split(root) = tree.root() else {
        panic!("a split")
    };
    assert_eq!(root.shares(), [0.75, 0.25]);
    let Node::Split(column) = &root.children()[1] else {
        panic!("a split")
    };
    assert!(!column.side_by_side());
    assert_eq!(column.shares(), [0.25, 0.75]);
}

#[test]
fn a_restored_split_numbers_its_panes_from_one() {
    let file: serde_json::Value = serde_json::from_str(STORED).unwrap();
    let stored: Layout = serde_json::from_value(file["tabs"].clone()).unwrap();
    let mut tree = PaneTree::<usize>::new();
    let built = tree.build(&stored.tabs[0].root);
    let numbers: Vec<usize> = built.iter().map(|(id, _)| id.number()).collect();
    assert_eq!(
        numbers,
        [1, 2, 3],
        "the first pane is pane 1, as in a window split by hand"
    );
    assert_eq!(tree.all(), [pane(1), pane(2), pane(3)]);
    assert_eq!(tree.active(), pane(1));
    assert!(tree.pane(pane(1)).is_some_and(|first| first.count() == 0));
    assert_eq!(
        tree.add_beside(pane(3), Side::Below),
        Some(pane(4)),
        "the next pane follows on"
    );
}

#[test]
fn panes_whose_documents_do_not_open_again_are_pruned() {
    let file: serde_json::Value = serde_json::from_str(STORED).unwrap();
    let stored: Layout = serde_json::from_value(file["tabs"].clone()).unwrap();
    let mut tree = PaneTree::<usize>::new();
    let built = tree.build(&stored.tabs[0].root);
    assert_eq!(built.len(), 3);
    tree.insert(0, Place::Into(built[0].0, None));
    tree.prune();
    assert_eq!(tree.all(), [built[0].0]);
    assert!(!tree.is_split());
    let mut full = with(&["a"]);
    assert!(
        full.build(&stored.tabs[0].root).is_empty(),
        "only an empty tree is divided"
    );
}

#[test]
fn a_tree_keeps_only_documents_that_open_again_and_no_preview() {
    let mut tree = with(&["a", "b", "c"]);
    tree.select("b");
    tree.insert(
        "p",
        Place::Beside {
            anchor: "b",
            side: Side::Right,
            companion: true,
        },
    );
    let page = |url: &str| document(json!({"kind": "browser", "url": url}));
    let reopen = |doc: &str| (doc != "a").then(|| page(doc));
    let kept = tree.to_layout(&|doc| reopen(doc));
    assert_eq!(
        kept,
        Some(layout::Node::Pane(layout::Pane {
            documents: vec![page("b"), page("c")],
            selected: 0,
        }))
    );
    assert_eq!(Tree::new().to_layout(&|doc| reopen(doc)), None);
}

fn document(value: serde_json::Value) -> Tab {
    value
}
