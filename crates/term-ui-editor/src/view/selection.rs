//! What follows a move of the selection (SN-T6): the revealed match goes and the further carets follow.

use term_base::signpost::name;

use super::EditorTextView;
use super::system::step;
use crate::multi;

impl EditorTextView {
    /// The selection moved and settled.
    pub(super) fn selection_moved(&self) {
        step(name!("selection: found match"), || self.clear_found());
        step(name!("selection: carets"), || multi::selection_changed(self));
    }
}
