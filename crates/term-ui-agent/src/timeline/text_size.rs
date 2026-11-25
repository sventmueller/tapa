//! The timeline at another text size: every card is built anew and measured again.

use super::Timeline;

impl Timeline {
    /// Builds the cards anew at the text size the user chose; a hidden timeline does so once it shows again.
    pub(crate) fn text_size_changed(&self) {
        self.heights.borrow_mut().clear();
        self.kept.clear();
        let following = self.follows();
        if !self.is_visible() {
            if !self.pending.replace(true) {
                self.pending_follow.set(following);
            }
            *self.behind.borrow_mut() = None;
            return;
        }
        self.corrections.borrow_mut().clear();
        self.table.reloadData();
        if following {
            self.scroll_to_end();
        }
    }
}
