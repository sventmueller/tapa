//! The rows' heights: estimated, measured as their cards are built, and corrected in the table afterwards.

use std::time::SystemTime;

use objc2::rc::Retained;
use objc2_app_kit::{NSAnimationContext, NSTableCellView, NSTableRowView, NSView};
use objc2_foundation::NSSize;
use term_thread::{Entry, EntryId};

use super::{BOTTOM, Correction, TOP, TRAILING, Timeline};
use crate::cards::{self, Ctx};
use crate::common::util::{activate, main_soon};
use crate::row::TimelineRow;

/// How many rows beyond each edge of the screen get their cards built ahead.
const AHEAD: usize = 8;

impl Timeline {
    pub(crate) fn row_height(&self, row: usize) -> f64 {
        let Some(entry) = self.entry(row) else { return 20.0 };
        self.known_height(&entry)
    }

    /// The height published to AppKit for `entry`, which stays until a correction tells the table of another one; an
    /// estimate at the current width for an entry without one.
    pub(super) fn known_height(&self, entry: &Entry) -> f64 {
        if let Some((_, height)) = self.heights.borrow().get(&entry.id.get()) {
            return *height;
        }
        let width = self.width.get();
        let height = cards::estimate(entry, width - Self::leading(entry) - TRAILING) + TOP + BOTTOM;
        self.heights.borrow_mut().insert(entry.id.get(), (width, height));
        height
    }

    pub(crate) fn row_view(&self, row: usize) -> Retained<NSTableRowView> {
        let mark = cards::mark(&self.list.borrow(), row);
        Retained::into_super(TimelineRow::new(self.mtm, mark))
    }

    /// The card of `row` in a cell, measured; its row is corrected afterwards when the table had another height.
    pub fn cell(&self, row: usize) -> Option<Retained<NSView>> {
        let entry = self.entry(row)?;
        let width = self.table_width();
        self.fit_column(width);
        let before = self.known_height(&entry);
        if let Some((cell, height)) = self.kept.get(&entry, width) {
            self.record_height(row, entry.id, width, before, height);
            return Some(cell);
        }
        let leading = Self::leading(&entry);
        let running = self.live.borrow().len();
        let card = {
            let thread = self.thread.borrow();
            let expanded = self.expanded.borrow();
            let ctx = Ctx {
                mtm: self.mtm,
                width: width - leading - TRAILING,
                thread: &thread,
                expanded: &expanded,
                target: &self.target,
                cwd: &self.cwd,
                inputs: &self.inputs,
                live: &self.live,
                nested: false,
                now: SystemTime::now(),
            };
            let (card, figures) = crate::figures::collect(|| cards::build(&ctx, &entry));
            self.figures.borrow_mut().insert(entry.id, figures);
            card
        };
        self.built.set(self.built.get() + 1);
        if !self.prefetching.get() {
            self.prefetch_soon();
        }
        let cell = NSTableCellView::new(self.mtm);
        cell.addSubview(&card);
        card.setTranslatesAutoresizingMaskIntoConstraints(false);
        activate([
            card.leadingAnchor()
                .constraintEqualToAnchor_constant(&cell.leadingAnchor(), leading),
            card.trailingAnchor()
                .constraintEqualToAnchor_constant(&cell.trailingAnchor(), -TRAILING),
            card.topAnchor()
                .constraintEqualToAnchor_constant(&cell.topAnchor(), TOP),
        ]);
        let bottom = card
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&cell.bottomAnchor(), -BOTTOM);
        // Below required, so a row the table still has at its estimated height stretches the card, not the rules.
        bottom.setPriority(999.0);
        bottom.setActive(true);
        let height = measure(&cell, width);
        self.record_height(row, entry.id, width, before, height);
        let cell = Retained::into_super(cell);
        // A running time only advances while its label is listed in `live`, which a card scrolled away leaves.
        if self.live.borrow().len() == running {
            self.kept
                .put(&entry, width, height, &cell, |cell| cell.window().is_some());
        }
        Some(cell)
    }

    /// Keeps delegate heights stable until AppKit is notified; geometry queries may otherwise corrupt its cache.
    pub(super) fn record_height(&self, row: usize, id: EntryId, width: f64, before: f64, after: f64) {
        if (width - self.width.get()).abs() >= 0.5 {
            return;
        }
        if (after - before).abs() <= 0.5 {
            self.corrections.borrow_mut().remove(&row);
            self.heights.borrow_mut().insert(id.get(), (width, before));
            return;
        }
        self.corrections
            .borrow_mut()
            .entry(row)
            .and_modify(|correction| {
                correction.width = width;
                correction.after = after;
            })
            .or_insert(Correction {
                id,
                width,
                before,
                after,
            });
        self.defer();
    }

    /// Flushes the height corrections on the main thread's next turn and, once the frame after it is committed, builds
    /// the cards next to the screen when asked to.
    fn defer(&self) {
        if self.deferred.replace(true) {
            return;
        }
        let key = self.key;
        main_soon(move || {
            if let Some(tab) = crate::get(key) {
                let timeline = tab.timeline();
                timeline.deferred.set(false);
                timeline.flush_corrections();
                if timeline.prefetch_due.replace(false) {
                    crate::common::util::after_frame(move || tab.timeline().prefetch());
                }
            }
        });
    }

    /// Asks for the cards next to the screen to be built ahead.
    fn prefetch_soon(&self) {
        self.prefetch_due.set(true);
        self.defer();
    }

    /// Builds the card of the nearest of up to [`AHEAD`] rows below and above the screen that is not kept yet, one a
    /// frame, so a scroll finds it built instead of building it in the frame that shows it; a card taller than two
    /// screens costs more than a frame to build, so it waits for the scroll as before. While the timeline follows its
    /// end, what streams in is built as it shows, so nothing is built ahead.
    fn prefetch(&self) {
        if self.pending.get() || self.follows() {
            return;
        }
        let range = self.table.rowsInRect(self.table.visibleRect());
        let (first, shown) = (range.location, range.length);
        if shown == 0 {
            return;
        }
        let end = first + shown;
        let width = self.table_width();
        let tallest = 2.0 * self.table.visibleRect().size.height;
        let below = (end..end + AHEAD).map(Some);
        let above = (first.saturating_sub(AHEAD)..first).rev().map(Some);
        let nearest = below
            .zip(above.chain(std::iter::repeat(None)))
            .flat_map(|(b, a)| [b, a])
            .flatten();
        for row in nearest {
            let Some(entry) = self.entry(row) else { continue };
            if self.kept.get(&entry, width).is_none() && self.known_height(&entry) <= tallest {
                self.prefetching.set(true);
                drop(self.cell(row));
                self.prefetching.set(false);
                // A card that is not kept, such as one with a running time, would be built again every frame.
                if self.kept.get(&entry, width).is_some() {
                    self.prefetch_soon();
                }
                return;
            }
        }
    }

    /// Gives the rows rebuilt outside AppKit's own row building their height before the window draws again; the
    /// deferred flush would leave one frame with each changed card in its old row.
    pub(super) fn correct_now(&self) {
        for _ in 0..4 {
            if self.corrections.borrow().is_empty() {
                break;
            }
            self.flush_corrections();
        }
    }

    pub(crate) fn flush_corrections(&self) {
        let corrections = std::mem::take(&mut *self.corrections.borrow_mut());
        if corrections.is_empty() {
            return;
        }
        let following = self.follows();
        let visible = self.scroll.contentView().bounds();
        let mut shift = 0.0;
        let set = objc2_foundation::NSMutableIndexSet::new();
        for (row, correction) in &corrections {
            if self.rows.borrow().get(*row) != Some(&correction.id) {
                continue;
            }
            let rect = self.table.rectOfRow(isize::try_from(*row).unwrap_or(0));
            if rect.origin.y + rect.size.height <= visible.origin.y + 1.0 {
                shift += correction.after - correction.before;
            }
            set.addIndex(*row);
        }
        // All old rectangles must be read before publishing any new heights.
        for (row, correction) in corrections {
            if self.rows.borrow().get(row) == Some(&correction.id) {
                self.heights
                    .borrow_mut()
                    .insert(correction.id.get(), (correction.width, correction.after));
            }
        }
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(0.0);
        self.table.noteHeightOfRowsWithIndexesChanged(&set);
        NSAnimationContext::endGrouping();
        if following {
            self.follow_end();
        } else if shift.abs() > 0.5 {
            self.shift_by(shift);
        }
    }

    /// Makes the one column as wide as the table; a table does not do that for a column it was given in code.
    pub(super) fn fit_column(&self, width: f64) {
        let columns = self.table.tableColumns();
        if let Some(column) = columns.firstObject()
            && (column.width() - width).abs() > 0.5
        {
            column.setWidth(width);
        }
    }
}

/// The height `cell` needs at `width`.
fn measure(cell: &NSTableCellView, width: f64) -> f64 {
    cell.setFrameSize(NSSize::new(width, 100.0));
    let constraint = cell.widthAnchor().constraintEqualToConstant(width);
    constraint.setActive(true);
    let fitting = cell.fittingSize();
    constraint.setActive(false);
    fitting.height.ceil()
}
