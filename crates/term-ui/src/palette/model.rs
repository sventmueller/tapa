//! The command palette without AppKit (F102): which entries a query finds in what order, a chord as the menus write
//! it, and which rows of the list show.

/// How well a query matches a text; higher is better.
type Score = i64;

/// The entries that match `query`, the best first, as indexes into `entries`; each entry is its title and words that
/// find it too but do not show. An empty query keeps every entry in its order.
pub fn rank(query: &str, entries: &[(&str, &str)]) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut scored: Vec<(Score, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, (title, extra))| Some((score(&query, title, extra)?, index)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, index)| index).collect()
}

/// How well `query`, in lower case, matches an entry: the title before the hidden words, a whole match before a
/// prefix, a word's start before the middle, every word of the query before the initials of the title's words before
/// scattered letters.
fn score(query: &str, title: &str, extra: &str) -> Option<Score> {
    let title = title.to_lowercase();
    let extra = extra.to_lowercase().replace('_', " ");
    let length = |text: &str| Score::try_from(text.chars().count()).unwrap_or(Score::MAX);
    if title == query {
        return Some(10_000);
    }
    if title.starts_with(query) {
        return Some(9_000 - length(&title));
    }
    if let Some(at) = word_start(&title, query, 1) {
        return Some(8_000 - at);
    }
    if let Some(at) = title.find(query) {
        return Some(7_000 - length(&title[..at]));
    }
    if let Some(at) = word_start(&extra, query, 0) {
        return Some(6_000 - at);
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() > 1 && words.iter().all(|word| title.contains(word) || extra.contains(word)) {
        return Some(5_000);
    }
    if let Some(skipped) = initials(query, &title) {
        return Some(4_500 - skipped);
    }
    if let Some(spread) = scattered(query, &title) {
        return Some(4_000 - spread);
    }
    extra.contains(query).then_some(3_000)
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
fn initials(query: &str, text: &str) -> Option<Score> {
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).peekable();
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
fn scattered(query: &str, text: &str) -> Option<Score> {
    let wanted: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let mut next = 0;
    let mut first = None;
    let mut spread: Score = 0;
    let mut previous: Option<char> = None;
    for (at, c) in text.chars().enumerate() {
        if next == wanted.len() {
            break;
        }
        if c == wanted[next] {
            first.get_or_insert(at);
            let starts_word = previous.is_none_or(|p| !p.is_alphanumeric());
            spread -= if starts_word { 2 } else { 0 };
            next += 1;
        } else if first.is_some() {
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
