//! Taking in a new state of the thread, and catching up on what changed while the timeline was hidden.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use objc2_foundation::NSIndexSet;
use term_thread::{Change, EntryKind, Lane, Thread};

use super::{BOTTOM, TOP, Timeline, hidden_in_focus, is_quiet_hook, is_running_hook, owner_row, shown, ui_summary};
use crate::cards;
use crate::common::util::live;
use crate::row::TimelineRow;

impl Timeline {
    /// Takes in a new state; content-only changes keep the row index and untouched cards.
    pub(crate) fn apply(&self, thread: Arc<Thread>, changes: &[Change]) {
        let following = self.follows() || self.count() == 0;
        let structural = self.structure_changed(&thread, changes);
        let old_len = self.count();
        let appended = !structural || self.append_rows(&thread, changes) || self.reindex(&thread);
        if changes.contains(&Change::Cleared) {
            self.kept.clear();
        }
        *self.thread.borrow_mut() = thread;
        let reload = self.changed_rows(changes);
        if !structural && reload.is_empty() {
            return;
        }
        if !self.is_visible() {
            if !self.pending.replace(true) {
                self.pending_follow.set(following);
            }
            let mut behind = self.behind.borrow_mut();
            if !appended || changes.contains(&Change::Cleared) {
                *behind = None;
            }
            for row in reload {
                self.forget_card(row);
                if let Some(rows) = behind.as_mut() {
                    rows.insert(row);
                }
            }
            return;
        }
        if self.is_parked() {
            self.unpark();
            self.refresh_marks();
            return;
        }
        if self.pending.get() {
            self.catch_up();
            self.refresh_marks();
            return;
        }
        let reloaded = !appended || changes.contains(&Change::Cleared);
        if reloaded {
            for row in &reload {
                self.forget_card(*row);
            }
            self.table.reloadData();
        } else {
            if self.count() > old_len {
                let added = NSIndexSet::indexSetWithIndexesInRange(objc2_foundation::NSRange::new(
                    old_len,
                    self.count() - old_len,
                ));
                self.table
                    .insertRowsAtIndexes_withAnimation(&added, objc2_app_kit::NSTableViewAnimationOptions::empty());
            }
            self.refresh_rows(reload.into_iter().filter(|i| *i < old_len));
        }
        self.correct_now();
        self.refresh_marks();
        if following && reloaded {
            self.scroll_to_end();
        } else if following {
            self.follow_end();
        }
    }

    pub(super) fn structure_changed(&self, thread: &Thread, changes: &[Change]) -> bool {
        if self.thread.borrow().entries(&Lane::Main).len() != thread.entries(&Lane::Main).len() {
            return true;
        }
        changes.iter().any(|change| match change {
            Change::Cleared | Change::Inserted { lane: Lane::Main, .. } | Change::Removed { lane: Lane::Main, .. } => {
                true
            }
            Change::Updated { lane: Lane::Main, id } => {
                let new = thread.entry(*id);
                let old_thread = self.thread.borrow();
                let old = old_thread.entry(*id);
                match (old, new) {
                    (Some(old), Some(new)) => {
                        is_quiet_hook(old) != is_quiet_hook(new)
                            || is_running_hook(old) != is_running_hook(new)
                            || matches!(old.kind, EntryKind::UserMessage(_))
                                != matches!(new.kind, EntryKind::UserMessage(_))
                            || (self.focus.get() && hidden_in_focus(old) != hidden_in_focus(new))
                    }
                    _ => true,
                }
            }
            _ => false,
        })
    }

    /// Rebuilds the filtered index only for structural edits, in linear time.
    pub(super) fn reindex(&self, thread: &Thread) -> bool {
        let list = shown(thread, self.focus.get());
        let new: Vec<_> = list.iter().map(|e| e.id).collect();
        let positions: HashMap<_, _> = new.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let old = self.rows.borrow();
        let appended = new.starts_with(&old);
        for id in old.iter().filter(|id| !positions.contains_key(id)) {
            self.kept.forget(*id);
            self.inputs.borrow_mut().forget(id.get());
            self.heights.borrow_mut().remove(&id.get());
            self.figures.borrow_mut().remove(id);
        }
        drop(old);
        // A correction waiting for its flush would otherwise be lost, and its card stay squeezed into the old height.
        let corrections = std::mem::take(&mut *self.corrections.borrow_mut());
        *self.corrections.borrow_mut() = corrections
            .into_values()
            .filter_map(|correction| positions.get(&correction.id).map(|row| (*row, correction)))
            .collect();
        *self.rows.borrow_mut() = new;
        *self.positions.borrow_mut() = positions;
        *self.list.borrow_mut() = list;
        appended
    }

    pub(super) fn changed_rows(&self, changes: &[Change]) -> HashSet<usize> {
        let thread = self.thread.borrow();
        let positions = self.positions.borrow();
        let mut list = self.list.borrow_mut();
        let mut reload = HashSet::new();
        for change in changes {
            match change {
                Change::Updated { lane: Lane::Main, id } => {
                    if let Some(&i) = positions.get(id) {
                        if let Some((_, index)) = thread.place(*id) {
                            list[i] = thread.entries(&Lane::Main)[index].clone();
                        }
                        reload.insert(i);
                    }
                }
                Change::Updated {
                    lane: Lane::Subagent(key),
                    ..
                }
                | Change::Inserted {
                    lane: Lane::Subagent(key),
                    ..
                }
                | Change::Removed {
                    lane: Lane::Subagent(key),
                    ..
                }
                | Change::Subagent(key) => {
                    if let Some(i) = owner_row(&thread, &positions, key) {
                        reload.insert(i);
                    }
                }
                _ => {}
            }
        }
        reload
    }

    pub(super) fn is_visible(&self) -> bool {
        !self.scroll.isHiddenOrHasHiddenAncestor()
            && self.scroll.window().is_some_and(|w| crate::common::panel::in_front(&w))
    }

    /// A background tab catches up once, when it becomes visible again.
    pub(crate) fn show_pending(&self) {
        self.unpark();
        if self.is_visible() && self.pending.get() {
            self.catch_up();
        }
    }

    /// Shows what changed while the timeline was hidden: the cards that changed or were appended, and at another width
    /// the cards in view built anew at the width it has now; and the end if it showed it before.
    pub(super) fn catch_up(&self) {
        // A native tab has just adopted its sidebar; hydrate at the final viewport width.
        if let Some(window) = self.scroll.window() {
            window.layoutIfNeeded();
        }
        self.pending.set(false);
        let behind = self.behind.replace(Some(BTreeSet::new()));
        let width = self.table_width();
        let widened = (width - self.width.get()).abs() >= 0.5;
        if widened {
            self.commit_width(width);
        }
        if let Some(rows) = behind {
            self.catch_up_rows(rows, widened);
            return;
        }
        // Without the rows that changed every kept card may be out of date.
        self.kept.clear();
        self.corrections.borrow_mut().clear();
        self.table.reloadData();
        if self.pending_follow.get() {
            self.scroll_to_end();
        }
    }

    /// Tells the table of the rows appended while it was hidden and builds the cards of `changed` again, as a timeline
    /// in view takes the same changes, and after a change of width those it has loaded too.
    pub(super) fn catch_up_rows(&self, changed: BTreeSet<usize>, widened: bool) {
        let before = usize::try_from(self.table.numberOfRows()).unwrap_or(0);
        if self.count() > before {
            let added =
                NSIndexSet::indexSetWithIndexesInRange(objc2_foundation::NSRange::new(before, self.count() - before));
            self.table
                .insertRowsAtIndexes_withAnimation(&added, objc2_app_kit::NSTableViewAnimationOptions::empty());
        }
        let changed = changed.into_iter().filter(|row| *row < before);
        if widened {
            let mut rows: BTreeSet<usize> = changed.collect();
            rows.extend(self.loaded_rows());
            self.reload_rows(rows.into_iter());
        } else {
            self.refresh_rows(changed);
        }
        self.correct_now();
        if self.pending_follow.get() {
            self.scroll_to_end();
        }
    }

    /// Makes `width` the one the rows are laid out at; rows out of view keep the height they have until their card is
    /// built at it.
    fn commit_width(&self, width: f64) {
        self.fit_column(width);
        self.width.set(width);
        self.corrections.borrow_mut().clear();
    }

    /// The rows the table has a view for, those in view and those it prepared around them.
    fn loaded_rows(&self) -> Vec<usize> {
        let rows = std::cell::RefCell::new(Vec::new());
        let block = block2::StackBlock::new(|_: std::ptr::NonNull<objc2_app_kit::NSTableRowView>, row: isize| {
            crate::common::util::guarded((), || {
                if let Ok(row) = usize::try_from(row) {
                    rows.borrow_mut().push(row);
                }
            });
        });
        self.table.enumerateAvailableRowViewsUsingBlock(&block);
        rows.into_inner()
    }

    /// Lets go of the kept card of `row`, whose entry changed in a way its pointer does not show.
    fn forget_card(&self, row: usize) {
        if let Some(id) = self.rows.borrow().get(row) {
            self.kept.forget(*id);
        }
    }

    pub(super) fn refresh_rows(&self, rows: impl Iterator<Item = usize>) {
        let mut reload = Vec::new();
        for row in rows {
            if !self.update_stream(row) {
                reload.push(row);
            }
        }
        self.reload_rows(reload.into_iter());
    }

    pub(super) fn update_stream(&self, row: usize) -> bool {
        let Some(entry) = self.entry(row) else { return false };
        let EntryKind::AgentMessage(message) = &entry.kind else {
            return false;
        };
        if !message.streaming || message.markdown.trim().is_empty() {
            return false;
        }
        let Some(cell) = self
            .table
            .viewAtColumn_row_makeIfNecessary(0, isize::try_from(row).unwrap_or(0), false)
        else {
            return false;
        };
        let Some(text) = cell
            .subviews()
            .firstObject()
            .and_then(|v| v.downcast::<crate::text::CardText>().ok())
        else {
            return false;
        };
        let before = self.known_height(&entry);
        let (updated, figures) = crate::figures::collect(|| text.update_markdown(&message.markdown));
        if updated {
            self.figures.borrow_mut().insert(entry.id, figures);
            ui_summary(&text, &entry, &self.thread.borrow());
            // `measure` sizes a detached cell. Resizing a live table cell would invalidate other rows.
            let height = text.intrinsicContentSize().height.ceil() + TOP + BOTTOM;
            self.record_height(row, entry.id, self.table_width(), before, height);
        }
        true
    }

    pub(super) fn reload_rows(&self, rows: impl Iterator<Item = usize>) {
        let set = objc2_foundation::NSMutableIndexSet::new();
        for row in rows {
            self.forget_card(row);
            set.addIndex(row);
        }
        if set.count() > 0 {
            self.table
                .reloadDataForRowIndexes_columnIndexes(&set, &NSIndexSet::indexSetWithIndex(0));
        }
    }

    /// Builds the card of entry `id` anew.
    pub(crate) fn reload_entry(&self, id: u64) {
        if let Some(row) = self
            .positions
            .borrow()
            .iter()
            .find_map(|(entry, row)| (entry.get() == id).then_some(*row))
        {
            self.reload_rows(std::iter::once(row));
            self.correct_now();
        }
    }

    /// Gives the rows in view the marks of their place, which change as rows are added after them.
    pub(super) fn refresh_marks(&self) {
        let entries = self.list.borrow();
        let range = self.table.rowsInRect(self.table.visibleRect());
        for row in range.location..range.location + range.length {
            let Some(view) = self
                .table
                .rowViewAtRow_makeIfNecessary(isize::try_from(row).unwrap_or(0), false)
            else {
                continue;
            };
            if let Ok(view) = view.downcast::<TimelineRow>() {
                view.set_mark(cards::mark(&entries, row));
            }
        }
    }

    /// Builds the cards anew, as formulas and diagrams they show as code were drawn.
    pub(crate) fn figures_arrived(&self, keys: &[splat_render::js::Key]) {
        let rows: Vec<_> = self
            .figures
            .borrow()
            .iter()
            .filter(|(_, wanted)| wanted.iter().any(|key| keys.contains(key)))
            .filter_map(|(id, _)| self.positions.borrow().get(id).copied())
            .collect();
        if rows.is_empty() {
            return;
        }
        for row in &rows {
            self.forget_card(*row);
        }
        if !self.is_visible() {
            if !self.pending.replace(true) {
                self.pending_follow.set(self.follows());
            }
            return;
        }
        self.reload_rows(rows.into_iter());
        self.correct_now();
        self.follow_end();
    }

    /// Lays the cards out anew after the width changed.
    pub(crate) fn width_changed(&self) {
        let width = self.table_width();
        // `record_height` drops measurements half a point off the committed width, so a smaller change must commit.
        if (width - self.width.get()).abs() < 0.5 || self.table.inLiveResize() {
            return;
        }
        // The column follows the table by itself, so the rows in view take the width and only the heights wait.
        if live::dragging(&self.table) {
            if !self.after_drag.replace(true) {
                let key = self.key;
                live::after_drag(move || {
                    if let Some(tab) = crate::get(key) {
                        tab.timeline().after_drag.set(false);
                        tab.timeline().width_changed();
                    }
                });
            }
            return;
        }
        let following = self.follows();
        if !self.is_visible() {
            if !self.pending.replace(true) {
                self.pending_follow.set(following);
            }
            return;
        }
        if self.is_parked() {
            return self.unpark();
        }
        self.commit_width(width);
        self.reload_rows(self.loaded_rows().into_iter());
        self.correct_now();
        if following {
            self.scroll_to_end();
        }
    }
}
