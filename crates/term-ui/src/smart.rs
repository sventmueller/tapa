//! Smart selection (F112): what a double click selects before it falls back to a word: a URL, a path, a hash, an IP
//! address or a quoted string around the character clicked.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4};

use term_session::{Cell, CellFlags, Point};

use crate::links::{self, Line};

/// The longest quoted string a double click selects, in characters.
const LONGEST_QUOTE: usize = 400;

/// The first and last cell of what a double click at `point` selects, if a recogniser knows it.
pub(crate) fn range_at(rows: &[Vec<Cell>], point: Point) -> Option<(Point, Point)> {
    let line = Line::around(rows, point);
    let at = line.index_of(point)?;
    let (start, end) = recognise(&line.chars, at)?;
    let first = line.cells[start];
    let mut last = line.cells[end - 1];
    let wide = rows
        .get(last.line)
        .and_then(|row| row.get(last.column))
        .is_some_and(|cell| cell.flags.contains(CellFlags::WIDE));
    if wide {
        last.column += 1;
    }
    Some((first, last))
}

/// The characters `start..end` of `chars` a double click at `at` selects: a URL, an IP address, a path, a hash, or a
/// quoted string, in that order.
pub(crate) fn recognise(chars: &[char], at: usize) -> Option<(usize, usize)> {
    if chars.get(at).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let url = links::urls(chars)
        .into_iter()
        .find(|&(start, end)| (start..end).contains(&at));
    url.or_else(|| ip(chars, at))
        .or_else(|| path(chars, at))
        .or_else(|| hash(chars, at))
        .or_else(|| quoted(chars, at))
}

/// The run of characters around `at` that `keep` accepts.
fn around(chars: &[char], at: usize, keep: impl Fn(char) -> bool) -> Option<(usize, usize)> {
    if !keep(*chars.get(at)?) {
        return None;
    }
    let mut start = at;
    while start > 0 && keep(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && keep(chars[end]) {
        end += 1;
    }
    Some((start, end))
}

/// A path with a slash, or a file name with an extension, with the `:line:column` after it.
fn path(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let breaks = |c: char| c.is_whitespace() || c.is_control() || "\"'`()<>[]{}|;,".contains(c);
    let (start, mut end) = around(chars, at, |c| !breaks(c))?;
    while end > start && ".:".contains(chars[end - 1]) {
        end -= 1;
    }
    if at >= end {
        return None;
    }
    let word: String = chars[start..end].iter().collect();
    let mut name = word.as_str();
    let mut numbers = 0;
    while numbers < 2
        && let Some((before, number)) = name.rsplit_once(':')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
    {
        name = before;
        numbers += 1;
    }
    let file = name.rsplit('/').next().unwrap_or(name);
    let extension = file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=8).contains(&ext.len())
            && ext.chars().all(char::is_alphanumeric)
            && ext.chars().any(char::is_alphabetic)
    });
    let slash = name.contains('/') && name.chars().any(char::is_alphanumeric) && !name.contains("//");
    (slash || name.starts_with('~') && name.len() > 1 || extension && numbers > 0).then_some((start, end))
}

/// A hexadecimal hash such as a commit's, 7 to 128 digits with at least one decimal digit, or a number after `0x`.
fn hash(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let (start, end) = around(chars, at, |c| c.is_ascii_alphanumeric())?;
    let word = &chars[start..end];
    if let [first, second, rest @ ..] = word
        && *first == '0'
        && matches!(second, 'x' | 'X')
        && !rest.is_empty()
        && rest.iter().all(char::is_ascii_hexdigit)
    {
        return Some((start, end));
    }
    let hex = (7..=128).contains(&word.len())
        && word.iter().all(char::is_ascii_hexdigit)
        && word.iter().any(char::is_ascii_digit);
    hex.then_some((start, end))
}

/// An IPv4 address with an optional port or prefix length, or an IPv6 address with an optional prefix length.
fn ip(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let (start, mut end) = around(chars, at, |c| c.is_ascii_hexdigit() || ".:/".contains(c))?;
    let mut start = start;
    while start < end && ".:/".contains(chars[start]) && !(chars[start] == ':' && chars.get(start + 1) == Some(&':')) {
        start += 1;
    }
    while end > start && ".:/".contains(chars[end - 1]) && !(chars[end - 1] == ':' && end >= 2 && chars[end - 2] == ':')
    {
        end -= 1;
    }
    if !(start..end).contains(&at) {
        return None;
    }
    let text: String = chars[start..end].iter().collect();
    let v4 = |text: &str| text.parse::<Ipv4Addr>().is_ok();
    let v6 = |text: &str| text.parse::<Ipv6Addr>().is_ok();
    let valid = match text.split_once('/') {
        Some((address, prefix)) => match prefix.parse::<u8>() {
            Ok(bits) if prefix.chars().all(|c| c.is_ascii_digit()) => {
                v4(address) && bits <= 32 || v6(address) && bits <= 128
            }
            _ => false,
        },
        None => v4(&text) || text.parse::<SocketAddrV4>().is_ok() || v6(&text),
    };
    valid.then_some((start, end))
}

/// What a pair of quotes around `at` holds, the quotes left out; an apostrophe inside a word is no quote.
fn quoted(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let word = |i: usize| chars.get(i).is_some_and(|c| c.is_alphanumeric());
    let mut best: Option<(usize, usize)> = None;
    for quote in ['"', '\'', '`'] {
        if chars[at] == quote {
            continue;
        }
        let Some(open) = (0..at).rev().find(|&i| chars[i] == quote) else {
            continue;
        };
        let Some(close) = (at + 1..chars.len()).find(|&i| chars[i] == quote) else {
            continue;
        };
        let opens = open == 0 || !word(open - 1);
        let closes = !word(close + 1);
        let inside = (open + 1, close);
        if opens && closes && close - open - 1 <= LONGEST_QUOTE && best.is_none_or(|b| inside.1 - inside.0 < b.1 - b.0)
        {
            best = Some(inside);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a double click in `text` under the `^` of `at` selects.
    fn picked(text: &str, at: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let at = at.find('^').expect("a mark");
        recognise(&chars, at).map(|(start, end)| chars[start..end].iter().collect())
    }

    #[test]
    fn a_url_is_selected_whole_without_the_punctuation_after_it() {
        assert_eq!(
            picked("see https://example.com/a?b=c, then", "              ^").as_deref(),
            Some("https://example.com/a?b=c")
        );
    }

    #[test]
    fn a_path_keeps_its_slashes_dots_and_line_number() {
        assert_eq!(
            picked("error at src/term-ui/main.rs:12:5: oops", "          ^").as_deref(),
            Some("src/term-ui/main.rs:12:5")
        );
        assert_eq!(picked("cd ~/Repos/tapa.", "       ^").as_deref(), Some("~/Repos/tapa"));
        assert_eq!(picked("  lib.rs:40 here", "  ^").as_deref(), Some("lib.rs:40"));
        assert_eq!(picked("a plain word", "  ^"), None);
    }

    #[test]
    fn a_hash_needs_seven_hex_digits_and_a_decimal_one() {
        assert_eq!(
            picked("commit 3f2a9c1d0e (HEAD)", "         ^").as_deref(),
            Some("3f2a9c1d0e")
        );
        assert_eq!(picked("at 0x7ffee4a0 now", "     ^").as_deref(), Some("0x7ffee4a0"));
        assert_eq!(picked("a deadbeef here", "   ^"), None, "letters only are a word");
        assert_eq!(picked("id 12ab", "   ^"), None, "too short");
    }

    #[test]
    fn an_ip_address_keeps_its_port_or_prefix() {
        assert_eq!(
            picked("from 192.168.1.20:22 ok", "      ^").as_deref(),
            Some("192.168.1.20:22")
        );
        assert_eq!(picked("net 10.0.0.0/8.", "     ^").as_deref(), Some("10.0.0.0/8"));
        assert_eq!(
            picked("v6 fe80::1ff:fe23:4567:890a here", "    ^").as_deref(),
            Some("fe80::1ff:fe23:4567:890a")
        );
        assert_eq!(picked("loop ::1 ok", "      ^").as_deref(), Some("::1"));
        assert_eq!(picked("ver 1.2.3 ok", "     ^"), None, "a version is no address");
        assert_eq!(picked("bad 300.1.1.1", "     ^"), None);
    }

    #[test]
    fn a_quoted_string_is_selected_without_its_quotes() {
        assert_eq!(
            picked("echo \"hello big world\" done", "            ^").as_deref(),
            Some("hello big world")
        );
        assert_eq!(picked("it's Tom's day", "  ^"), None, "apostrophes are no quotes");
        assert_eq!(picked("run `make test` now", "       ^").as_deref(), Some("make test"));
    }

    #[test]
    fn a_blank_selects_nothing_smart() {
        assert_eq!(picked("a  b", " ^"), None);
    }

    #[test]
    fn a_range_on_the_screen_ends_on_the_second_cell_of_a_wide_character() {
        let mut row: Vec<Cell> = "\"日 x\""
            .chars()
            .map(|ch| Cell {
                ch,
                zero_width: vec![],
                fg: term_session::Rgb::default(),
                bg: term_session::Rgb::default(),
                underline_color: None,
                flags: CellFlags::default(),
                hyperlink: None,
            })
            .collect();
        row[1].flags = CellFlags::WIDE;
        let mut spacer = row[1].clone();
        spacer.ch = ' ';
        spacer.flags = CellFlags::WIDE_SPACER;
        row.insert(2, spacer);
        let range = range_at(&[row], Point { line: 0, column: 4 });
        assert_eq!(
            range,
            Some((Point { line: 0, column: 1 }, Point { line: 0, column: 4 }))
        );
    }
}
