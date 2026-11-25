//! The panes of a tab as a tree without a window (E12): parts side by side or one above the other, nested as deep as
//! the user splits, the pane commands act on, the one that fills the tab for now, and the rules of splitting, closing
//! and moving. The terminal keeps one session per pane; the tree, copied from Dango, can hold more behind one another.
//! The app's views mirror it, keyed by the ids it hands out; `D` is the id of a session.

pub mod layout;
mod stored;
#[cfg(test)]
mod tests;

/// A pane's number in its window tab, from 1 on, which stays while the pane lives and names it to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PaneId(usize);

impl PaneId {
    /// The pane's number.
    pub fn number(self) -> usize {
        self.0
    }

    /// The id of the pane numbered `number`, which a tree may or may not hold.
    pub fn from_number(number: usize) -> PaneId {
        PaneId(number)
    }
}

/// A split's id in its tree, which stays while the split lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SplitId(usize);

/// Where a new pane goes beside another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// To its right, the two side by side.
    Right,
    /// Below it, the two one above the other.
    Below,
}

/// Where a document goes in a window tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place<D> {
    /// Into the active pane, after its document in front.
    Active,
    /// Into a pane at an index, or after its document in front.
    Into(PaneId, Option<usize>),
    /// Into a new pane at `side` of the pane that holds `anchor`; `companion` for a preview that follows it (E59).
    Beside {
        /// The document the new pane goes beside.
        anchor: D,
        /// Where the new pane goes.
        side: Side,
        /// Whether the new pane holds the preview of `anchor`, which follows it and counts as no split.
        companion: bool,
    },
}

/// What a dragged tab is over: the bar of a pane at a place among its tabs, or a pane's documents, near its right or
/// bottom edge or inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The bar of the pane, where the tab would go to the index.
    Bar(PaneId, usize),
    /// The documents of the pane, at the edge `Some(side)` or inside.
    Content(PaneId, Option<Side>),
}

/// Where a dragged tab lands when let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Into the pane at an index, or after its document in front.
    Into(PaneId, Option<usize>),
    /// Into a new pane at a side of the pane.
    Beside(PaneId, Side),
    /// Nowhere; the tab stays.
    Nowhere,
}

/// A part of the tree: a pane, or parts side by side or one above the other.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A pane.
    Pane(PaneId),
    /// Parts next to one another.
    Split(Split),
}

impl Node {
    /// The panes in the node, left to right and top to bottom.
    pub(crate) fn panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes(&self, into: &mut Vec<PaneId>) {
        match self {
            Node::Pane(pane) => into.push(*pane),
            Node::Split(split) => split.children.iter().for_each(|child| child.collect_panes(into)),
        }
    }

    /// The splits in the node, each before the ones inside it.
    pub fn splits(&self) -> Vec<&Split> {
        let mut splits = Vec::new();
        self.collect_splits(&mut splits);
        splits
    }

    fn collect_splits<'a>(&'a self, into: &mut Vec<&'a Split>) {
        if let Node::Split(split) = self {
            into.push(split);
            split.children.iter().for_each(|child| child.collect_splits(into));
        }
    }

    /// The way from this node to the one `matches` picks, as child indices; empty for this node itself.
    fn path_to(&self, matches: &dyn Fn(&Node) -> bool) -> Option<Vec<usize>> {
        if matches(self) {
            return Some(Vec::new());
        }
        let Node::Split(split) = self else { return None };
        split.children.iter().enumerate().find_map(|(index, child)| {
            child.path_to(matches).map(|mut path| {
                path.insert(0, index);
                path
            })
        })
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((first, rest)) => match self {
                Node::Split(split) => split.children.get_mut(*first)?.at_mut(rest),
                Node::Pane(_) => None,
            },
        }
    }

    fn split_at_mut(&mut self, path: &[usize]) -> Option<&mut Split> {
        match self.at_mut(path)? {
            Node::Split(split) => Some(split),
            Node::Pane(_) => None,
        }
    }
}

/// Parts side by side or one above the other, each with its share of the room along the split.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    id: SplitId,
    side_by_side: bool,
    children: Vec<Node>,
    /// Each child's share, adding up to 1.
    shares: Vec<f64>,
}

impl Split {
    fn new(id: SplitId, side_by_side: bool) -> Split {
        Split {
            id,
            side_by_side,
            children: Vec::new(),
            shares: Vec::new(),
        }
    }

    /// The split's id.
    pub fn id(&self) -> SplitId {
        self.id
    }

    /// Whether the parts stand left to right, else top to bottom.
    pub fn side_by_side(&self) -> bool {
        self.side_by_side
    }

    /// The parts, in their order.
    pub fn children(&self) -> &[Node] {
        &self.children
    }

    /// Each part's share of the room, adding up to 1.
    pub fn shares(&self) -> &[f64] {
        &self.shares
    }

    /// Puts `node` at `index` with `share` of the room, taken from the others in proportion; the first child takes
    /// all of it.
    fn insert(&mut self, index: usize, node: Node, share: f64) {
        let index = index.min(self.children.len());
        self.children.insert(index, node);
        let share = if self.shares.is_empty() {
            1.0
        } else {
            share.clamp(0.05, 0.95)
        };
        for other in &mut self.shares {
            *other *= 1.0 - share;
        }
        self.shares.insert(index.min(self.shares.len()), share);
    }

    /// Takes the child at `index` out; its share goes to the neighbour before it, else the one after.
    fn remove(&mut self, index: usize) -> Node {
        let node = self.children.remove(index);
        let freed = if index < self.shares.len() {
            self.shares.remove(index)
        } else {
            0.0
        };
        let neighbour = index.checked_sub(1).or((!self.shares.is_empty()).then_some(0));
        if let Some(neighbour) = neighbour.and_then(|at| self.shares.get_mut(at)) {
            *neighbour += freed;
        }
        node
    }

    /// Takes `shares`, scaled to add up to 1, unless they do not fit the children: one each, finite and above 0.
    fn set_shares(&mut self, shares: &[f64]) -> bool {
        let total: f64 = shares.iter().sum();
        let fits = shares.len() == self.children.len()
            && total > 0.0
            && shares.iter().all(|share| share.is_finite() && *share > 0.0);
        if fits {
            self.shares = shares.iter().map(|share| share / total).collect();
        }
        fits
    }
}

/// One pane: its documents behind one another, the one in front showing.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane<D> {
    id: PaneId,
    documents: Vec<D>,
    selected: usize,
    /// The document whose preview the pane shows beside it, which it follows and closes with (E59).
    companion_of: Option<D>,
}

impl<D: Copy + Eq> Pane<D> {
    fn new(id: PaneId) -> Pane<D> {
        Pane {
            id,
            documents: Vec::new(),
            selected: 0,
            companion_of: None,
        }
    }

    /// The documents, in the order of the pane's tab bar.
    pub fn documents(&self) -> &[D] {
        &self.documents
    }

    /// The index of the document in front.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The document in front.
    pub fn front(&self) -> Option<D> {
        self.documents.get(self.selected).copied()
    }

    /// How many documents the pane holds.
    pub fn count(&self) -> usize {
        self.documents.len()
    }

    /// Whether the pane holds `document`.
    pub(crate) fn holds(&self, document: D) -> bool {
        self.documents.contains(&document)
    }

    /// The document whose preview the pane holds, if it is a preview's pane.
    pub fn companion_of(&self) -> Option<D> {
        self.companion_of
    }

    /// Puts `document` at `index`, or after the one in front, and brings it to the front.
    fn insert(&mut self, document: D, index: Option<usize>) {
        let count = self.documents.len();
        let index = index
            .unwrap_or(if count == 0 { 0 } else { self.selected + 1 })
            .min(count);
        self.documents.insert(index, document);
        self.select_index(index);
    }

    /// Takes `document` out; the one after it comes to the front, else the one before.
    fn take(&mut self, document: D) -> bool {
        let Some(index) = self.documents.iter().position(|doc| *doc == document) else {
            return false;
        };
        self.documents.remove(index);
        let selected = self.selected;
        let next = if selected > index || (selected == index && index >= self.documents.len()) {
            selected.saturating_sub(1)
        } else {
            selected
        };
        self.select_index(next);
        true
    }

    fn select(&mut self, document: D) -> bool {
        let index = self.documents.iter().position(|doc| *doc == document);
        if let Some(index) = index {
            self.select_index(index);
        }
        index.is_some()
    }

    /// Brings the document `step` places from the one in front to the front, wrapping around; whether there was one
    /// other to bring.
    fn select_relative(&mut self, step: isize) -> bool {
        let count = self.documents.len();
        if count < 2 {
            return false;
        }
        self.select_index(step_index(self.selected, step, count));
        true
    }

    fn select_index(&mut self, index: usize) {
        self.selected = index.min(self.documents.len().saturating_sub(1));
    }
}

/// The index `step` places from `index` among `count`, wrapping around; `count` is above 0.
fn step_index(index: usize, step: isize, count: usize) -> usize {
    let count = isize::try_from(count).unwrap_or(isize::MAX);
    let index = isize::try_from(index).unwrap_or(0);
    usize::try_from((index + step).rem_euclid(count)).unwrap_or(0)
}

/// The documents of an unsplit tree that get a native tab of their own once it is no longer split (E60): those of its
/// main pane but the one in front, in the pane's order around it, each with the document of the preview that follows
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spread<D> {
    /// The document in front, which keeps the window tab.
    pub front: D,
    /// The documents before it, which get tabs before its tab.
    pub before: Vec<(D, Option<D>)>,
    /// The documents after it, which get tabs after its tab.
    pub after: Vec<(D, Option<D>)>,
}

/// The panes of a window tab.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneTree<D> {
    root: Node,
    /// The panes in the order they were made.
    panes: Vec<Pane<D>>,
    /// The pane commands act on: the one the keyboard focus was in last.
    active: PaneId,
    /// The pane that fills the window tab alone for now, the others kept hidden.
    zoomed: Option<PaneId>,
    next_pane: usize,
    next_split: usize,
}

impl<D: Copy + Eq> Default for PaneTree<D> {
    fn default() -> Self {
        PaneTree::new()
    }
}

impl<D: Copy + Eq> PaneTree<D> {
    /// One empty pane, numbered 1.
    pub fn new() -> PaneTree<D> {
        let first = PaneId(1);
        PaneTree {
            root: Node::Pane(first),
            panes: vec![Pane::new(first)],
            active: first,
            zoomed: None,
            next_pane: 2,
            next_split: 1,
        }
    }

    /// The tree's top node.
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Every pane, left to right and top to bottom.
    pub fn all(&self) -> Vec<PaneId> {
        self.root.panes()
    }

    /// The pane `id`, if the tree holds it.
    pub fn pane(&self, id: PaneId) -> Option<&Pane<D>> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane<D>> {
        self.panes.iter_mut().find(|pane| pane.id == id)
    }

    /// The split `id`, if the tree holds it.
    pub fn split(&self, id: SplitId) -> Option<&Split> {
        self.root.splits().into_iter().find(|split| split.id == id)
    }

    /// The documents, pane by pane.
    pub fn documents(&self) -> Vec<D> {
        self.all()
            .into_iter()
            .filter_map(|id| self.pane(id))
            .flat_map(|pane| pane.documents.iter().copied())
            .collect()
    }

    /// Whether no pane holds a document.
    pub fn is_empty(&self) -> bool {
        self.panes.iter().all(|pane| pane.documents.is_empty())
    }

    /// The pane that holds `document`.
    pub fn pane_of(&self, document: D) -> Option<PaneId> {
        self.panes.iter().find(|pane| pane.holds(document)).map(|pane| pane.id)
    }

    /// The pane commands act on.
    pub fn active(&self) -> PaneId {
        self.active
    }

    /// Makes `pane` the one commands act on, if the tree holds it.
    pub fn set_active(&mut self, pane: PaneId) {
        if self.pane(pane).is_some() {
            self.active = pane;
        }
    }

    /// The document commands act on: the one in front in the active pane.
    pub fn focused(&self) -> Option<D> {
        self.pane(self.active)?.front()
    }

    /// The first pane that is not a preview's, which holds the documents of a tab that is not split.
    pub fn main(&self) -> PaneId {
        let all = self.all();
        all.iter()
            .copied()
            .find(|id| self.pane(*id).is_some_and(|pane| pane.companion_of.is_none()))
            .or_else(|| all.first().copied())
            .unwrap_or(self.active)
    }

    /// The pane of the preview that follows the document `owner`.
    pub(crate) fn companion_of(&self, owner: D) -> Option<PaneId> {
        self.all()
            .into_iter()
            .find(|id| self.pane(*id).is_some_and(|pane| pane.companion_of == Some(owner)))
    }

    /// Whether the user split the window tab: two panes or more besides the previews that follow a document.
    pub fn is_split(&self) -> bool {
        self.panes.iter().filter(|pane| pane.companion_of.is_none()).count() > 1
    }

    /// Puts `document` where `place` says and returns its pane. A document the tree holds already stays where it is
    /// and comes to the front there, as a document is open at most once in a window (E145).
    pub fn insert(&mut self, document: D, place: Place<D>) -> PaneId {
        if let Some(pane) = self.pane_of(document) {
            self.select(document);
            self.active = pane;
            return pane;
        }
        let pane = match place {
            Place::Into(pane, index) if self.pane(pane).is_some() => {
                if let Some(into) = self.pane_mut(pane) {
                    into.insert(document, index);
                }
                return pane;
            }
            Place::Beside {
                anchor,
                side,
                companion,
            } => match self.pane_of(anchor).and_then(|beside| self.add_beside(beside, side)) {
                Some(pane) => {
                    if let Some(new) = self.pane_mut(pane) {
                        new.companion_of = companion.then_some(anchor);
                    }
                    pane
                }
                None => self.active_for_documents(),
            },
            Place::Active | Place::Into(..) => self.active_for_documents(),
        };
        if let Some(into) = self.pane_mut(pane) {
            into.insert(document, None);
        }
        pane
    }

    /// The active pane, or the one whose document a preview in the active pane follows, which a preview never shares.
    fn active_for_documents(&self) -> PaneId {
        self.pane(self.active)
            .and_then(|pane| pane.companion_of)
            .and_then(|owner| self.pane_of(owner))
            .unwrap_or(self.active)
    }

    /// Takes `document` out; a pane left empty goes, unless it is the last. Whether the tree held it.
    pub fn take(&mut self, document: D) -> bool {
        let Some(id) = self.pane_of(document) else {
            return false;
        };
        let emptied = self
            .pane_mut(id)
            .is_some_and(|pane| pane.take(document) && pane.count() == 0);
        if emptied {
            self.remove_pane(id);
        }
        true
    }

    /// Brings `document` to the front of its pane; whether the tree holds it.
    pub fn select(&mut self, document: D) -> bool {
        let Some(id) = self.pane_of(document) else {
            return false;
        };
        self.pane_mut(id).is_some_and(|pane| pane.select(document))
    }

    /// Brings `document` to the front of its pane and makes that pane the active one.
    pub fn mark_focused(&mut self, document: D) -> bool {
        let Some(id) = self.pane_of(document) else {
            return false;
        };
        self.select(document);
        self.active = id;
        true
    }

    /// Brings the document `step` places from the one in front of the active pane to the front, wrapping around; the
    /// document in front then, if the pane held another.
    pub fn select_relative(&mut self, step: isize) -> Option<D> {
        let pane = self.pane_mut(self.active)?;
        pane.select_relative(step).then(|| pane.front()).flatten()
    }

    fn new_pane(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        self.panes.push(Pane::new(id));
        id
    }

    fn new_split(&mut self, side_by_side: bool) -> Split {
        let id = SplitId(self.next_split);
        self.next_split += 1;
        Split::new(id, side_by_side)
    }

    fn path_of_pane(&self, pane: PaneId) -> Option<Vec<usize>> {
        self.root.path_to(&|node| matches!(node, Node::Pane(id) if *id == pane))
    }

    /// A new, empty pane at `side` of `pane`: it joins a split that runs that way and takes half of the room of
    /// `pane`, else `pane` and the new one split the place of `pane` half and half. Shows every pane again.
    pub(crate) fn add_beside(&mut self, pane: PaneId, side: Side) -> Option<PaneId> {
        let mut path = self.path_of_pane(pane)?;
        let side_by_side = side == Side::Right;
        let Some(index) = path.pop() else {
            self.zoomed = None;
            let new = self.new_pane();
            let mut split = self.new_split(side_by_side);
            let old = std::mem::replace(&mut self.root, Node::Pane(new));
            split.insert(0, old, 1.0);
            split.insert(1, Node::Pane(new), 0.5);
            self.root = Node::Split(split);
            return Some(new);
        };
        let same_way = self.root.split_at_mut(&path)?.side_by_side == side_by_side;
        self.zoomed = None;
        let new = self.new_pane();
        let split = (!same_way).then(|| self.new_split(side_by_side));
        let parent = self.root.split_at_mut(&path)?;
        match split {
            None => {
                let mut shares = parent.shares.clone();
                let half = shares.get(index).copied().unwrap_or(0.5) / 2.0;
                parent.insert(index + 1, Node::Pane(new), 0.5);
                if let Some(own) = shares.get_mut(index) {
                    *own = half;
                }
                shares.insert((index + 1).min(shares.len()), half);
                parent.set_shares(&shares);
            }
            Some(mut split) => {
                split.insert(0, Node::Pane(pane), 1.0);
                split.insert(1, Node::Pane(new), 0.5);
                parent.children[index] = Node::Split(split);
            }
        }
        Some(new)
    }

    /// Takes the empty `pane` away and closes up its split; the last pane and one holding documents stay. When it was
    /// the active one, the last pane of the part before it becomes active, else the first. Whether it went.
    pub fn remove_pane(&mut self, pane: PaneId) -> bool {
        if self.panes.len() < 2 || self.pane(pane).is_none_or(|pane| pane.count() > 0) {
            return false;
        }
        let Some(mut path) = self.path_of_pane(pane) else {
            return false;
        };
        let Some(index) = path.pop() else { return false };
        let Some(parent) = self.root.split_at_mut(&path) else {
            return false;
        };
        parent.remove(index);
        let neighbour = parent.children.get(index.saturating_sub(1)).cloned();
        self.panes.retain(|other| other.id != pane);
        if self.zoomed == Some(pane) {
            self.zoomed = None;
        }
        if self.active == pane {
            let inside = neighbour.map(|node| node.panes()).unwrap_or_default();
            if let Some(next) = inside.last().copied().or_else(|| self.all().first().copied()) {
                self.active = next;
            }
        }
        self.close_up(&path);
        true
    }

    /// Closes `pane` without closing its documents (E145): they move behind those of the pane before it, else the one
    /// after, which becomes active and keeps its document in front. The pane they went into; none for a preview's
    /// pane, which closes with its documents, and for the last pane.
    pub fn close_pane(&mut self, pane: PaneId) -> Option<PaneId> {
        let closing = self.pane(pane).filter(|closing| closing.companion_of.is_none())?;
        let documents = closing.documents.clone();
        let closing_front = closing.front();
        let heir = self.heir(pane)?;
        let front = self.pane(heir)?.front().or(closing_front);
        for document in documents {
            self.move_document(document, heir, Some(usize::MAX));
        }
        self.remove_pane(pane);
        if let Some(front) = front {
            self.select(front);
        }
        self.active = heir;
        Some(heir)
    }

    /// The pane that takes in the documents of `pane` as it closes: the nearest one before it that is no preview's,
    /// else the nearest one after.
    fn heir(&self, pane: PaneId) -> Option<PaneId> {
        let all = self.all();
        let at = all.iter().position(|id| *id == pane)?;
        let takes = |id: &&PaneId| self.pane(**id).is_some_and(|other| other.companion_of.is_none());
        all[..at]
            .iter()
            .rev()
            .find(takes)
            .or_else(|| all[at + 1..].iter().find(takes))
            .copied()
    }

    /// Puts the only child of the split at `path` in its place, merging it into the split around when it runs the
    /// same way, its parts taking their shares of its share.
    fn close_up(&mut self, path: &[usize]) {
        let Some(split) = self.root.split_at_mut(path) else {
            return;
        };
        if split.children.len() != 1 {
            return;
        }
        let child = split.remove(0);
        let Some((&index, parent_path)) = path.split_last() else {
            self.root = child;
            return;
        };
        let Some(parent) = self.root.split_at_mut(parent_path) else {
            return;
        };
        match child {
            Node::Split(inner) if inner.side_by_side == parent.side_by_side => {
                let own = parent.shares.get(index).copied().unwrap_or(0.5);
                let mut merged = parent.shares.clone();
                merged.remove(index.min(merged.len().saturating_sub(1)));
                parent.remove(index);
                for (offset, grandchild) in inner.children.into_iter().enumerate() {
                    parent.insert(index + offset, grandchild, 0.5);
                    let share = own * inner.shares.get(offset).copied().unwrap_or(0.5);
                    merged.insert((index + offset).min(merged.len()), share);
                }
                parent.set_shares(&merged);
            }
            child => parent.children[index] = child,
        }
    }

    /// Takes away the panes left empty, as documents of a stored layout did not open again.
    pub fn prune(&mut self) {
        for pane in self.all() {
            if self.pane(pane).is_some_and(|pane| pane.count() == 0) {
                self.remove_pane(pane);
            }
        }
    }

    /// Fills the window tab with `pane` alone for now, or shows the others again when it does already.
    pub fn toggle_zoom(&mut self, pane: PaneId) {
        if self.pane(pane).is_none() {
            return;
        }
        self.zoomed = if self.zoomed == Some(pane) { None } else { Some(pane) };
    }

    /// Whether a pane fills the window tab alone for now.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }

    /// Whether `pane` shows: the one that fills the tab, else every pane but a preview's whose document is behind
    /// another in its pane.
    pub fn is_shown(&self, pane: PaneId) -> bool {
        if let Some(zoomed) = self.zoomed {
            return zoomed == pane;
        }
        self.pane(pane).is_some_and(|pane| {
            pane.companion_of.is_none_or(|owner| {
                self.pane_of(owner)
                    .and_then(|of| self.pane(of))
                    .is_none_or(|of| of.front() == Some(owner))
            })
        })
    }

    /// Shows every pane again and makes the pane `step` places from the active one among those that show the active
    /// one, wrapping around and leaving out an empty preview's; that pane, unless there are fewer than two.
    pub fn step_pane(&mut self, step: isize) -> Option<PaneId> {
        self.zoomed = None;
        let candidates: Vec<PaneId> = self
            .all()
            .into_iter()
            .filter(|id| {
                self.pane(*id)
                    .is_some_and(|pane| pane.companion_of.is_none() || pane.count() > 0)
            })
            .collect();
        if candidates.len() < 2 || step == 0 {
            return None;
        }
        let index = candidates.iter().position(|id| *id == self.active).unwrap_or(0);
        let next = (1..=candidates.len())
            .map(|times| {
                let times = isize::try_from(times).unwrap_or(1);
                candidates[step_index(index, step.signum() * times, candidates.len())]
            })
            .find(|id| self.is_shown(*id))?;
        self.active = next;
        Some(next)
    }

    /// Moves `document` into `to` at `index`, or after its document in front, makes `to` active and drops the pane
    /// it leaves empty; along its own pane it only moves to `index`. Whether it moved.
    pub(crate) fn move_document(&mut self, document: D, to: PaneId, index: Option<usize>) -> bool {
        let Some(from) = self.pane_of(document) else {
            return false;
        };
        if self.pane(to).is_none() {
            return false;
        }
        if let Some(pane) = self.pane_mut(from) {
            pane.take(document);
        }
        if let Some(pane) = self.pane_mut(to) {
            pane.insert(document, index);
        }
        self.active = to;
        if from != to && self.pane(from).is_some_and(|pane| pane.count() == 0) {
            self.remove_pane(from);
        }
        true
    }

    /// Moves `document` into a new pane at `side` of its own, which keeps its other documents; the new pane, unless
    /// `document` is alone in its pane.
    pub fn split_off(&mut self, document: D, side: Side) -> Option<PaneId> {
        let from = self.pane_of(document)?;
        if self.pane(from).is_none_or(|pane| pane.count() < 2) {
            return None;
        }
        let new = self.add_beside(from, side)?;
        self.move_document(document, new, None);
        Some(new)
    }

    /// Splits the active pane at `side` (E145): while it holds other documents, its document in front moves into the
    /// new pane; else the new pane opens empty and active, for the next document. The new pane and the document that
    /// moved.
    pub fn split_active(&mut self, side: Side) -> Option<(PaneId, Option<D>)> {
        let active = self.active;
        let pane = self.pane(active)?;
        match pane.front() {
            Some(front) if pane.count() > 1 => self.split_off(front, side).map(|new| (new, Some(front))),
            _ => {
                let new = self.add_beside(active, side)?;
                self.active = new;
                Some((new, None))
            }
        }
    }

    /// Where `document` lands when its tab is let go over `hit`: a pane's bar takes it at the place; a pane's edge
    /// gives it a new pane there, unless it is alone in its own pane; another pane's inside takes it after its
    /// document in front.
    pub fn landing(&self, document: D, hit: Option<Hit>) -> Landing {
        let Some(from) = self.pane_of(document) else {
            return Landing::Nowhere;
        };
        match hit {
            None => Landing::Nowhere,
            Some(Hit::Bar(pane, index)) => Landing::Into(pane, Some(index)),
            Some(Hit::Content(pane, side)) => {
                let own = pane == from;
                let more = self.pane(from).is_some_and(|pane| pane.count() > 1);
                match side {
                    Some(side) if !own || more => Landing::Beside(pane, side),
                    None if !own => Landing::Into(pane, None),
                    _ => Landing::Nowhere,
                }
            }
        }
    }

    /// Puts `document` where `landing` says; whether it moved.
    pub fn drop_at(&mut self, document: D, landing: Landing) -> bool {
        match landing {
            Landing::Into(to, index) => self.move_document(document, to, index),
            Landing::Beside(pane, side) => self
                .add_beside(pane, side)
                .is_some_and(|new| self.move_document(document, new, None)),
            Landing::Nowhere => false,
        }
    }

    /// Takes the shares a drag of a divider of the split `id` gave its parts; whether they fit.
    pub fn set_shares(&mut self, id: SplitId, shares: &[f64]) -> bool {
        let path = self
            .root
            .path_to(&|node| matches!(node, Node::Split(split) if split.id == id));
        path.and_then(|path| self.root.split_at_mut(&path))
            .is_some_and(|split| split.set_shares(shares))
    }

    /// The documents that get a native tab of their own once the tree is no longer split; none while it is, or while
    /// its main pane holds one document or none.
    pub fn spread(&self) -> Option<Spread<D>> {
        if self.is_split() {
            return None;
        }
        let main = self.pane(self.main())?;
        let front = main.front()?;
        if main.count() < 2 {
            return None;
        }
        let with_preview = |document: D| {
            let preview = self
                .companion_of(document)
                .and_then(|pane| self.pane(pane))
                .and_then(Pane::front);
            (document, preview)
        };
        let at_front = main.selected;
        Some(Spread {
            front,
            before: main.documents[..at_front]
                .iter()
                .map(|doc| with_preview(*doc))
                .collect(),
            after: main.documents[at_front + 1..]
                .iter()
                .map(|doc| with_preview(*doc))
                .collect(),
        })
    }
}
