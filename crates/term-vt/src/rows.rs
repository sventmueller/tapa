//! The rows of a screen, top first, as Ghostty keeps them: the rows stay where they are and the screen holds their
//! order as small indices, so scrolling, also inside a region, moves four bytes a row instead of the rows.

use std::collections::VecDeque;
use std::ops::{Index, IndexMut, Range};

use crate::cell::Row;

/// The rows of a screen, top first.
#[derive(Debug, Clone, Default)]
pub struct Rows {
    order: VecDeque<u32>,
    slots: Vec<Row>,
}

impl Rows {
    /// The number of rows.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Row `line` from the top.
    pub fn get(&self, line: usize) -> Option<&Row> {
        self.order.get(line).map(|&slot| &self.slots[slot as usize])
    }

    /// Row `line` from the top, to change.
    pub fn get_mut(&mut self, line: usize) -> Option<&mut Row> {
        let slot = *self.order.get(line)?;
        self.slots.get_mut(slot as usize)
    }

    /// The rows, top first.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Row> + ExactSizeIterator {
        self.order.iter().map(|&slot| &self.slots[slot as usize])
    }

    /// Every row to change, in no particular order.
    pub fn each_mut(&mut self) -> impl Iterator<Item = &mut Row> {
        self.slots.iter_mut()
    }

    /// Adds `row` at the bottom.
    pub fn push_back(&mut self, row: Row) {
        let slot = self.slot_for(row);
        self.order.push_back(slot);
    }

    /// Puts `row` at `line`, the rows from there moving down.
    pub fn insert(&mut self, line: usize, row: Row) {
        let slot = self.slot_for(row);
        self.order.insert(line, slot);
    }

    /// Takes row `line` out, the rows below moving up.
    pub fn remove(&mut self, line: usize) -> Option<Row> {
        let slot = self.order.remove(line)? as usize;
        let last = u32::try_from(self.slots.len() - 1).unwrap_or(u32::MAX);
        let row = self.slots.swap_remove(slot);
        if let Some(moved) = self.order.iter_mut().find(|other| **other == last) {
            *moved = u32::try_from(slot).unwrap_or(u32::MAX);
        }
        Some(row)
    }

    /// Keeps the top `lines` rows.
    pub fn truncate(&mut self, lines: usize) {
        while self.order.len() > lines {
            let last = self.order.len() - 1;
            self.remove(last);
        }
    }

    /// Takes all rows, top first.
    pub fn take_all(&mut self) -> Vec<Row> {
        let mut slots: Vec<Option<Row>> = std::mem::take(&mut self.slots).into_iter().map(Some).collect();
        self.order
            .drain(..)
            .filter_map(|slot| slots[slot as usize].take())
            .collect()
    }

    /// The rows of `region` move up by `count`, the top ones going round to its bottom.
    pub fn rotate_up(&mut self, region: Range<usize>, count: usize) {
        if region.start == 0 && region.end == self.order.len() {
            self.order.rotate_left(count);
        } else {
            self.order.make_contiguous()[region].rotate_left(count);
        }
    }

    /// The rows of `region` move down by `count`, the bottom ones going round to its top.
    pub fn rotate_down(&mut self, region: Range<usize>, count: usize) {
        if region.start == 0 && region.end == self.order.len() {
            self.order.rotate_right(count);
        } else {
            self.order.make_contiguous()[region].rotate_right(count);
        }
    }

    fn slot_for(&mut self, row: Row) -> u32 {
        self.slots.push(row);
        u32::try_from(self.slots.len() - 1).unwrap_or(u32::MAX)
    }
}

impl Index<usize> for Rows {
    type Output = Row;

    fn index(&self, line: usize) -> &Row {
        &self.slots[self.order[line] as usize]
    }
}

impl IndexMut<usize> for Rows {
    fn index_mut(&mut self, line: usize) -> &mut Row {
        let slot = self.order[line] as usize;
        &mut self.slots[slot]
    }
}

impl FromIterator<Row> for Rows {
    fn from_iter<I: IntoIterator<Item = Row>>(rows: I) -> Rows {
        let slots: Vec<Row> = rows.into_iter().collect();
        let order = (0..slots.len())
            .map(|slot| u32::try_from(slot).unwrap_or(u32::MAX))
            .collect();
        Rows { order, slots }
    }
}

impl From<Vec<Row>> for Rows {
    fn from(rows: Vec<Row>) -> Rows {
        rows.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::StyleId;

    fn rows(text: &str) -> Rows {
        text.chars()
            .map(|c| {
                let mut row = Row::new(1, StyleId(0));
                row.cells[0].c = c;
                row.touch(1);
                row
            })
            .collect()
    }

    fn text(rows: &Rows) -> String {
        rows.iter().map(|row| row.cells[0].c).collect()
    }

    #[test]
    fn rotating_moves_the_order_and_editing_keeps_it() {
        let mut screen = rows("abcdef");
        screen.rotate_up(0..6, 1);
        assert_eq!(text(&screen), "bcdefa");
        screen.rotate_up(1..4, 1);
        assert_eq!(text(&screen), "bdecfa");
        screen.rotate_down(0..6, 2);
        assert_eq!(text(&screen), "fabdec");
        assert_eq!(screen.remove(1).unwrap().cells[0].c, 'a');
        screen.insert(0, rows("x").remove(0).unwrap());
        assert_eq!(text(&screen), "xfbdec");
        screen.truncate(3);
        assert_eq!(text(&screen), "xfb");
        screen[2].cells[0].c = 'z';
        assert_eq!(
            screen.take_all().iter().map(|row| row.cells[0].c).collect::<String>(),
            "xfz"
        );
    }
}
