//! The one replacement that turns one state of the copy into another, worked out on ropes that share what did not
//! change.

use crate::Replacement;

/// The one replacement that turns `before` into `after`: what lies between their common start and common end. A rope
/// cloned before an edit shares every leaf the edit left alone, so leaves at the same address are skipped whole and
/// the work grows with the change, not with the file, which may run to megabytes (E25).
pub(crate) fn difference(before: &ropey::Rope, after: &ropey::Rope) -> Option<Replacement> {
    let prefix = common_start(before.chunks(), after.chunks());
    let (old_len, new_len) = (before.len_bytes(), after.len_bytes());
    if prefix == old_len && prefix == new_len {
        return None;
    }
    let most = old_len.min(new_len) - prefix;
    let suffix = common_start(
        before.chunks_at_byte(old_len).0.reversed().map(Reversed),
        after.chunks_at_byte(new_len).0.reversed().map(Reversed),
    )
    .min(most);
    let (start, end) = (char_floor(before, prefix), char_ceil(before, old_len - suffix));
    let new_end = char_ceil(after, new_len - (old_len - end));
    let new_start = char_floor(after, start);
    Some(Replacement {
        start: position(before, start),
        end: position(before, end),
        text: after.byte_slice(new_start..new_end).to_string(),
    })
}

/// A chunk read from its end, for comparing ropes backwards.
struct Reversed<'a>(&'a str);

/// Bytes read in one direction from a chunk of a rope.
trait Piece {
    fn address(&self) -> (*const u8, usize);
    fn byte(&self, index: usize) -> u8;
}

impl Piece for &str {
    fn address(&self) -> (*const u8, usize) {
        (self.as_ptr(), self.len())
    }

    fn byte(&self, index: usize) -> u8 {
        self.as_bytes()[index]
    }
}

impl Piece for Reversed<'_> {
    fn address(&self) -> (*const u8, usize) {
        (self.0.as_ptr(), self.0.len())
    }

    fn byte(&self, index: usize) -> u8 {
        self.0.as_bytes()[self.0.len() - 1 - index]
    }
}

/// How many bytes two runs of chunks share at their start; chunks at the same address count whole.
fn common_start<P: Piece>(mut a: impl Iterator<Item = P>, mut b: impl Iterator<Item = P>) -> usize {
    let (mut ca, mut cb) = (a.next(), b.next());
    let (mut ia, mut ib, mut same) = (0usize, 0usize, 0usize);
    loop {
        let (Some(pa), Some(pb)) = (ca.as_ref(), cb.as_ref()) else {
            return same;
        };
        if ia == 0 && ib == 0 && pa.address() == pb.address() {
            same += pa.address().1;
            (ca, cb) = (a.next(), b.next());
            continue;
        }
        if ia == pa.address().1 {
            (ca, ia) = (a.next(), 0);
            continue;
        }
        if ib == pb.address().1 {
            (cb, ib) = (b.next(), 0);
            continue;
        }
        if pa.byte(ia) != pb.byte(ib) {
            return same;
        }
        same += 1;
        ia += 1;
        ib += 1;
    }
}

/// The start of the character that byte `index` of `rope` lies in.
fn char_floor(rope: &ropey::Rope, index: usize) -> usize {
    rope.char_to_byte(rope.byte_to_char(index))
}

/// The start of the next character at or after byte `index` of `rope`.
fn char_ceil(rope: &ropey::Rope, index: usize) -> usize {
    let char = rope.byte_to_char(index);
    let floor = rope.char_to_byte(char);
    if floor == index {
        index
    } else {
        rope.char_to_byte(char + 1)
    }
}

/// The line and column in characters of byte `index` of `rope`, a character boundary.
fn position(rope: &ropey::Rope, index: usize) -> (usize, usize) {
    let char = rope.byte_to_char(index);
    let line = rope.char_to_line(char);
    (line, char - rope.line_to_char(line))
}
