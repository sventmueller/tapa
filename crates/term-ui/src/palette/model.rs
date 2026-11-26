//! The command palette without AppKit (F102): which entries a query finds in what order, a chord as the menus write
//! it, and which rows of the list show.

use std::cmp::Reverse;

/// How well a query matches a text; higher is better.
type Score = i64;

/// The entries that match `query`, the best first, as indexes into `entries`; each entry is its title and words that
/// find it too but do not show. An empty query keeps every entry in its order.
pub fn rank(query: &str, entries: &[(&str, &str)]) -> Vec<usize> {
    let query = RankQuery::new(query);
    if query.text.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut scored: Vec<_> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, (title, extra))| Some((Reverse(query.score(&RankText::new(title, extra))?), index)))
        .collect();
    scored.sort_unstable();
    scored.into_iter().map(|(_, index)| index).collect()
}

/// A candidate's searchable text, normalized once for repeated queries.
pub struct RankText {
    title: String,
    extra: String,
}

impl RankText {
    /// Prepares a title and additional search words.
    pub fn new(title: &str, extra: &str) -> Self {
        let mut extra = extra.to_lowercase();
        if extra.contains('_') {
            extra = extra.replace('_', " ");
        }
        Self {
            title: title.to_lowercase(),
            extra,
        }
    }
}

/// One query against any number of prepared catalogs.
pub struct RankQuery {
    text: String,
    letters: Vec<char>,
    several_words: bool,
}

impl RankQuery {
    /// Prepares the query once, including the letters used for fuzzy matching.
    pub fn new(query: &str) -> Self {
        let text = query.trim().to_lowercase();
        let letters = text.chars().filter(|c| !c.is_whitespace()).collect();
        let several_words = text.split_whitespace().nth(1).is_some();
        Self {
            text,
            letters,
            several_words,
        }
    }

    /// Returns the best matches in descending score order, breaking ties by original position.
    /// Returns none if the caller has superseded this search.
    pub fn best<'a>(
        &self,
        entries: impl Iterator<Item = &'a RankText>,
        limit: usize,
        cancelled: impl Fn() -> bool,
    ) -> Option<Vec<(Score, usize)>> {
        let mut scored = Vec::new();
        for (index, text) in entries.enumerate() {
            if index % 128 == 0 && cancelled() {
                return None;
            }
            if let Some(score) = self.score(text) {
                scored.push((Reverse(score), index));
            }
        }
        if cancelled() {
            return None;
        }
        if scored.len() > limit {
            scored.select_nth_unstable(limit);
            scored.truncate(limit);
        }
        scored.sort_unstable();
        Some(
            scored
                .into_iter()
                .map(|(Reverse(score), index)| (score, index))
                .collect(),
        )
    }

    /// Matches the title before the hidden words, a whole match before a prefix, a word's start before the middle,
    /// every word of the query before the initials of the title's words before scattered letters.
    fn score(&self, entry: &RankText) -> Option<Score> {
        let query = self.text.as_str();
        let title = entry.title.as_str();
        let extra = entry.extra.as_str();
        let length = |text: &str| Score::try_from(text.chars().count()).unwrap_or(Score::MAX);
        if title == query {
            return Some(10_000);
        }
        if title.starts_with(query) {
            return Some(9_000 - length(title));
        }
        if let Some(at) = word_start(title, query, 1) {
            return Some(8_000 - at);
        }
        if let Some(at) = title.find(query) {
            return Some(7_000 - length(&title[..at]));
        }
        if let Some(at) = word_start(extra, query, 0) {
            return Some(6_000 - at);
        }
        if self.several_words
            && query
                .split_whitespace()
                .all(|word| title.contains(word) || extra.contains(word))
        {
            return Some(5_000);
        }
        if let Some(skipped) = initials(&self.letters, title) {
            return Some(4_500 - skipped);
        }
        if let Some(spread) = scattered(&self.letters, title) {
            return Some(4_000 - spread);
        }
        extra.contains(query).then_some(3_000)
    }
}

/// Which word of `text` past the first `skip`, counted from 0, begins with `query`.
fn word_start(text: &str, query: &str, skip: usize) -> Option<Score> {
    let at = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .skip(skip)
        .position(|word| word.starts_with(query))?;
    Score::try_from(at).ok()
}

/// Whether the letters of `query` begin words of `text` in their order, and how many words they leave out between.
fn initials(query: &[char], text: &str) -> Option<Score> {
    let mut wanted = query.iter().copied().peekable();
    let mut skipped: Score = 0;
    let mut started = false;
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        let Some(next) = wanted.peek() else { break };
        if word.starts_with(*next) {
            wanted.next();
            started = true;
        } else if started {
            skipped += 1;
        }
    }
    wanted.peek().is_none().then_some(skipped)
}

/// Whether the letters of `query` appear in `text` in their order, and how far apart: the letters skipped between
/// the first and the last, fewer where they start words.
fn scattered(wanted: &[char], text: &str) -> Option<Score> {
    let mut next = 0;
    let mut started = false;
    let mut spread: Score = 0;
    let mut previous: Option<char> = None;
    for c in text.chars() {
        if next == wanted.len() {
            break;
        }
        if c == wanted[next] {
            started = true;
            let starts_word = previous.is_none_or(|p| !p.is_alphanumeric());
            spread -= if starts_word { 2 } else { 0 };
            next += 1;
        } else if started {
            spread += 1;
        }
        previous = Some(c);
    }
    (next == wanted.len()).then_some(spread.max(0))
}

/// A chord as the menus write it, such as `⇧⌘P`.
pub fn glyphs(chord: &term_config::Chord) -> String {
    let mut out = String::new();
    for (on, glyph) in [
        (chord.ctrl, '⌃'),
        (chord.alt, '⌥'),
        (chord.shift, '⇧'),
        (chord.cmd, '⌘'),
    ] {
        if on {
            out.push(glyph);
        }
    }
    let key = match chord.key.as_str() {
        "enter" => "↩",
        "tab" => "⇥",
        "space" => "Space",
        "backspace" => "⌫",
        "delete" => "⌦",
        "escape" => "⎋",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        "home" => "↖",
        "end" => "↘",
        "pageup" => "⇞",
        "pagedown" => "⇟",
        other => return out + &other.to_uppercase(),
    };
    out + key
}

/// The first of `shown` rows the list shows so that row `selected` of `count` is among them, moving as little as
/// it can from `first`.
pub(crate) fn first_shown(first: usize, selected: usize, shown: usize, count: usize) -> usize {
    let shown = shown.max(1);
    let first = if selected < first {
        selected
    } else if selected >= first + shown {
        selected + 1 - shown
    } else {
        first
    };
    first.min(count.saturating_sub(shown))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRIES: &[(&str, &str)] = &[
        ("Neuer Tab", "new tab Opens a tab."),
        (
            "Rechts teilen",
            "split right Splits the pane, the new one to the right.",
        ),
        ("Unten teilen", "split down Splits the pane, the new one below."),
        ("Tab schließen", "close tab Closes the tab with all its panes."),
        ("Suchen …", "find Searches the history."),
        (
            "Kopiermodus",
            "copy mode Moves over the history with Vim's keys to select and copy (F111).",
        ),
        ("Kopieren", "copy Copies the selection."),
    ];

    fn titles(query: &str) -> Vec<&'static str> {
        rank(query, ENTRIES).into_iter().map(|index| ENTRIES[index].0).collect()
    }

    #[test]
    fn an_empty_query_keeps_every_entry_in_its_order() {
        assert_eq!(rank("  ", ENTRIES), (0..ENTRIES.len()).collect::<Vec<_>>());
    }

    #[test]
    fn a_prefix_comes_before_a_word_inside_before_the_middle() {
        assert_eq!(titles("kopier")[..2], ["Kopieren", "Kopiermodus"]);
        assert_eq!(titles("tab"), ["Tab schließen", "Neuer Tab"]);
        assert_eq!(titles("teilen"), ["Rechts teilen", "Unten teilen"]);
        assert_eq!(titles("eilen"), ["Unten teilen", "Rechts teilen"]);
    }

    #[test]
    fn the_hidden_words_find_entries_in_any_language() {
        assert_eq!(titles("split right")[0], "Rechts teilen");
        assert_eq!(titles("split")[..2], ["Rechts teilen", "Unten teilen"]);
        assert_eq!(titles("history")[0], "Suchen …");
    }

    #[test]
    fn scattered_letters_find_an_entry_closest_together_first() {
        assert_eq!(titles("nt"), ["Unten teilen", "Neuer Tab"]);
        assert_eq!(titles("rt")[0], "Rechts teilen");
        assert!(titles("qqq").is_empty());
    }

    #[test]
    fn chords_read_as_the_menus_write_them() {
        let glyph = |text: &str| glyphs(&text.parse().unwrap());
        assert_eq!(glyph("cmd+shift+p"), "⇧⌘P");
        assert_eq!(glyph("ctrl+alt+cmd+enter"), "⌃⌥⌘↩");
        assert_eq!(glyph("cmd+pageup"), "⌘⇞");
        assert_eq!(glyph("cmd+f5"), "⌘F5");
        assert_eq!(glyph("cmd+["), "⌘[");
    }

    #[test]
    fn the_list_scrolls_as_little_as_shows_the_selected_row() {
        assert_eq!(first_shown(0, 3, 10, 50), 0);
        assert_eq!(first_shown(0, 12, 10, 50), 3);
        assert_eq!(first_shown(5, 2, 10, 50), 2);
        assert_eq!(first_shown(30, 31, 10, 12), 2);
        assert_eq!(first_shown(0, 0, 10, 3), 0);
    }
}
