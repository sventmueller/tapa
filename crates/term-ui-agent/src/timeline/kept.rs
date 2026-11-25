//! The cards of rows scrolled away, kept so scrolling back hands the table the same cell instead of building the card
//! again; a kept card counts only for the entry it was built from, at the width it was built for.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

use term_thread::{Entry, EntryId};

/// How many cards a timeline keeps, those in view included.
const KEPT: usize = 240;

struct Card<C> {
    /// Held so the pointer the card is matched by cannot be reused by another entry.
    entry: Arc<Entry>,
    width: f64,
    height: f64,
    cell: C,
    used: u64,
}

/// The cards built, by entry, as cells of type `C`.
pub(super) struct Kept<C> {
    cards: RefCell<HashMap<EntryId, Card<C>>>,
    clock: Cell<u64>,
}

impl<C> Default for Kept<C> {
    fn default() -> Self {
        Kept {
            cards: RefCell::new(HashMap::new()),
            clock: Cell::new(0),
        }
    }
}

impl<C: Clone> Kept<C> {
    /// The cell built for `entry` at `width` and its measured height, if it is kept.
    pub(super) fn get(&self, entry: &Arc<Entry>, width: f64) -> Option<(C, f64)> {
        let mut cards = self.cards.borrow_mut();
        let card = cards.get_mut(&entry.id)?;
        if !Arc::ptr_eq(&card.entry, entry) || (card.width - width).abs() >= 0.5 {
            return None;
        }
        card.used = self.tick();
        Some((card.cell.clone(), card.height))
    }

    /// Keeps `cell`, built for `entry` at `width`, `height` high; beyond [`KEPT`] lets go of the cards used longest
    /// ago that are not `shown`.
    pub(super) fn put(&self, entry: &Arc<Entry>, width: f64, height: f64, cell: &C, shown: impl Fn(&C) -> bool) {
        let used = self.tick();
        let mut cards = self.cards.borrow_mut();
        cards.insert(
            entry.id,
            Card {
                entry: entry.clone(),
                width,
                height,
                cell: cell.clone(),
                used,
            },
        );
        if cards.len() <= KEPT {
            return;
        }
        let mut away: Vec<(u64, EntryId)> = cards
            .iter()
            .filter(|(_, card)| !shown(&card.cell))
            .map(|(id, card)| (card.used, *id))
            .collect();
        away.sort_unstable_by_key(|(used, _)| *used);
        let over = cards.len() - KEPT * 3 / 4;
        for (_, id) in away.into_iter().take(over) {
            cards.remove(&id);
        }
    }

    /// Lets go of the card of `id`, which has to be built again.
    pub(super) fn forget(&self, id: EntryId) {
        self.cards.borrow_mut().remove(&id);
    }

    /// Lets go of every card.
    pub(super) fn clear(&self) {
        self.cards.borrow_mut().clear();
    }

    fn tick(&self) -> u64 {
        let now = self.clock.get() + 1;
        self.clock.set(now);
        now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_thread::{EntryKind, Lane, Thread, ThreadId, Update, UserMessage};

    fn entries(count: usize) -> Vec<Arc<Entry>> {
        let mut thread = Thread::new(ThreadId::from("t"), "test", "/tmp");
        for _ in 0..count {
            thread.apply(Update::Put {
                lane: Lane::Main,
                key: None,
                at: std::time::UNIX_EPOCH,
                entry: EntryKind::UserMessage(UserMessage::default()),
            });
        }
        thread.entries(&Lane::Main).to_vec()
    }

    #[test]
    fn a_card_is_kept_for_its_entry_at_its_width_only() {
        let kept = Kept::default();
        let entry = entries(1).remove(0);
        kept.put(&entry, 300.0, 40.0, &7, |_| false);
        assert_eq!(kept.get(&entry, 300.2), Some((7, 40.0)));
        assert_eq!(kept.get(&entry, 320.0), None, "another width");
        let changed = Arc::new((*entry).clone());
        assert_eq!(kept.get(&changed, 300.0), None, "the entry changed");
        kept.forget(entry.id);
        assert_eq!(kept.get(&entry, 300.0), None);
    }

    #[test]
    fn beyond_its_bound_the_cards_used_longest_ago_go_but_not_those_shown() {
        let kept = Kept::default();
        let all = entries(KEPT + 1);
        for (n, entry) in all.iter().enumerate() {
            kept.put(entry, 300.0, 40.0, &n, |n| *n < 10);
        }
        assert!(kept.cards.borrow().len() <= KEPT * 3 / 4);
        for (n, entry) in all.iter().enumerate().take(10) {
            assert_eq!(kept.get(entry, 300.0), Some((n, 40.0)), "shown card {n}");
        }
        assert_eq!(kept.get(&all[KEPT], 300.0), Some((KEPT, 40.0)), "the newest");
        assert_eq!(kept.get(&all[10], 300.0), None, "the oldest away");
    }
}
