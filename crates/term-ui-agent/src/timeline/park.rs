//! A timeline out of view for a while lets go of the cards it built; their heights stay, so coming back builds only
//! the cards in view, at the place it showed.

use std::rc::Rc;
use std::time::Duration;

use super::Timeline;
use crate::tab::AgentTab;

/// How long a timeline stays out of view before it lets go of its cards.
const PARK_AFTER: Duration = Duration::from_secs(30);

impl Timeline {
    /// Lets the timeline of `tab` let go of its cards once it stayed out of view, and build them again when it shows,
    /// which also starts a restored session that waited for it.
    pub(crate) fn watch(tab: &Rc<AgentTab>) {
        let (came, away) = (Rc::downgrade(tab), Rc::downgrade(tab));
        crate::common::panel::watch_view(
            tab.timeline().view(),
            move || {
                if let Some(tab) = came.upgrade() {
                    tab.timeline().unpark();
                    tab.wake_deferred();
                }
            },
            Some((
                PARK_AFTER,
                Box::new(move || {
                    if let Some(tab) = away.upgrade() {
                        tab.timeline().park();
                    }
                }),
            )),
        );
    }

    /// The rows the table shows: none while parked.
    pub(crate) fn table_rows(&self) -> usize {
        if self.parked.get() { 0 } else { self.count() }
    }

    /// Whether the timeline let go of its cards.
    pub(crate) fn is_parked(&self) -> bool {
        self.parked.get()
    }

    /// Out of view: the table lets go of its rows, after noting where it was and whether it followed the end.
    fn park(&self) {
        if self.parked.get() || self.is_visible() || self.count() == 0 {
            return;
        }
        if !self.pending.replace(true) {
            self.pending_follow.set(self.follows());
        }
        self.parked_at.set(self.scroll.contentView().bounds().origin.y);
        self.parked.set(true);
        self.table.reloadData();
        self.live.borrow_mut().clear();
        self.kept.clear();
        self.corrections.borrow_mut().clear();
        crate::common::util::give_back_memory_soon();
    }

    /// In view again: builds the cards in view, at the end if it followed it, else where it was.
    pub(super) fn unpark(&self) {
        if !self.parked.get() || !self.is_visible() {
            return;
        }
        self.parked.set(false);
        self.pending.set(true);
        *self.behind.borrow_mut() = None;
        let follows = self.pending_follow.get();
        self.catch_up();
        if !follows {
            self.table.tile();
            self.scroll_ours(self.parked_at.get());
        }
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
