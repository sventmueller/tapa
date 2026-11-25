//! Vim's own search and Ex prompt (E106, F11): the editor's find bar gives no matches or patterns, so `/`, `?`, `n`,
//! `N`, `*`, `#`, `:s` and `:%s` search here, with Rust's regular expressions. Where they differ from Vim's patterns,
//! such as `\v` or `\<`, is in the help (R24).

use modalkit::key::TerminalKey;
use regex::Regex;

use crate::Note;

/// The prompt being typed and the last search, for `n` and `N`.
#[derive(Debug, Default)]
pub(crate) struct Search {
    /// The prompt's first character and what follows it, while it is open.
    open: Option<(char, String)>,
    /// The last pattern and whether it searched forward.
    last: Option<(String, bool)>,
}

/// A match, from its first character to the one after its last, each a line and a column in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Found {
    pub(crate) start: (usize, usize),
    pub(crate) end: (usize, usize),
}

/// What a key in the prompt did.
pub(crate) enum Step {
    /// The prompt goes on, or was left.
    Typing,
    /// The search found this match.
    Jump(Found),
    /// An Ex command was entered.
    Command(String),
    /// Nothing was found, or the pattern is not valid.
    Failed(Note),
}

impl Search {
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// What the prompt shows, such as `/foo`, while it is open.
    pub(crate) fn shown(&self) -> Option<String> {
        self.open.as_ref().map(|(kind, typed)| format!("{kind}{typed}"))
    }

    /// Opens the prompt of `kind`, `/`, `?` or `:`.
    pub(crate) fn open(&mut self, kind: char) {
        self.open = Some((kind, String::new()));
    }

    /// Takes `key` while the prompt is open, in the text `text` gives with the cursor at `cursor`; only a search asks
    /// for the text, which in a large file costs a copy of it.
    pub(crate) fn key(&mut self, key: &TerminalKey, text: impl FnOnce() -> String, cursor: (usize, usize)) -> Step {
        let name = key.to_string();
        let Some((kind, typed)) = self.open.as_mut() else {
            return Step::Typing;
        };
        match name.as_str() {
            "<Esc>" | "<C-C>" | "<C-[>" => {
                self.open = None;
                Step::Typing
            }
            "<BS>" => {
                if typed.pop().is_none() {
                    self.open = None;
                }
                Step::Typing
            }
            "<Enter>" | "<CR>" => {
                let (kind, typed) = (*kind, std::mem::take(typed));
                self.open = None;
                if kind == ':' {
                    return Step::Command(typed);
                }
                let forward = kind == '/';
                let pattern = if typed.is_empty() {
                    match &self.last {
                        Some((pattern, _)) => pattern.clone(),
                        None => return Step::Failed(Note::NoPreviousPattern),
                    }
                } else {
                    typed
                };
                self.last = Some((pattern.clone(), forward));
                match find(&pattern, &text(), cursor, forward, 1) {
                    Ok(found) => Step::Jump(found),
                    Err(note) => Step::Failed(note),
                }
            }
            one if one.chars().count() == 1 => {
                typed.push_str(one);
                Step::Typing
            }
            "<Space>" => {
                typed.push(' ');
                Step::Typing
            }
            _ => Step::Typing,
        }
    }

    /// `n`, or `N` with `flip`, `count` times in `text` with the cursor at `cursor`.
    pub(crate) fn again(&self, flip: bool, count: usize, text: &str, cursor: (usize, usize)) -> Result<Found, Note> {
        let (pattern, forward) = self.last.clone().ok_or(Note::NoPreviousPattern)?;
        find(&pattern, text, cursor, forward != flip, count)
    }

    /// `*` or `#`, and `g*` or `g#` without `whole`: the word under the cursor `count` times forward or back, which
    /// `n` and `N` then repeat.
    pub(crate) fn word(
        &mut self,
        forward: bool,
        whole: bool,
        count: usize,
        text: &str,
        cursor: (usize, usize),
    ) -> Result<Found, Note> {
        let word = word_at(text, cursor).ok_or(Note::NoWordUnderCursor)?;
        let pattern = if whole {
            format!(r"\b{}\b", regex::escape(&word))
        } else {
            regex::escape(&word)
        };
        self.last = Some((pattern.clone(), forward));
        find(&pattern, text, cursor, forward, count)
    }
}

/// The `count`th next match of `pattern` in `text` after `cursor`, or before it backwards, wrapping around the end.
fn find(pattern: &str, text: &str, cursor: (usize, usize), forward: bool, count: usize) -> Result<Found, Note> {
    let regex = Regex::new(pattern).map_err(|error| invalid(pattern, &error))?;
    let matches: Vec<(usize, usize)> = regex.find_iter(text).map(|m| (m.start(), m.end())).collect();
    let mut at = byte_of(text, cursor);
    let mut found = None;
    for _ in 0..count.max(1) {
        let next = if forward {
            matches.iter().find(|m| m.0 > at).or_else(|| matches.first())
        } else {
            matches.iter().rev().find(|m| m.0 < at).or_else(|| matches.last())
        };
        let Some(&next) = next else {
            return Err(Note::PatternNotFound(pattern.to_string()));
        };
        at = next.0;
        found = Some(next);
    }
    let (start, end) = found.ok_or_else(|| Note::PatternNotFound(pattern.to_string()))?;
    Ok(Found {
        start: place_of(text, start),
        end: place_of(text, end),
    })
}

/// `:s/old/new/[g]` on the cursor's line `line` or `:%s/…` on every line of `text`: the text after it, or why not;
/// `None` for any other command.
pub(crate) fn substitute(command: &str, text: &str, line: usize) -> Option<Result<String, Note>> {
    let (whole, rest) = match command.strip_prefix('%') {
        Some(rest) => (true, rest),
        None => (false, command),
    };
    let rest = rest.strip_prefix('s')?;
    let separator = rest.chars().next().filter(|c| !c.is_alphanumeric())?;
    let parts: Vec<&str> = rest[separator.len_utf8()..].split(separator).collect();
    let (old, new) = (parts.first()?, parts.get(1).copied().unwrap_or_default());
    let global = parts.get(2).is_some_and(|flags| flags.contains('g'));
    let regex = match Regex::new(old) {
        Ok(regex) => regex,
        Err(error) => return Some(Err(invalid(old, &error))),
    };
    let replacement = new.replace('&', "$0");
    let mut matched = false;
    let lines: Vec<String> = text
        .split_inclusive('\n')
        .enumerate()
        .map(|(n, content)| {
            if !(whole || n == line) || !regex.is_match(content) {
                return content.to_string();
            }
            matched = true;
            if global {
                regex.replace_all(content, replacement.as_str()).into_owned()
            } else {
                regex.replace(content, replacement.as_str()).into_owned()
            }
        })
        .collect();
    if !matched {
        return Some(Err(Note::PatternNotFound((*old).to_string())));
    }
    Some(Ok(lines.concat()))
}

fn invalid(pattern: &str, error: &regex::Error) -> Note {
    Note::InvalidPattern {
        pattern: pattern.to_string(),
        detail: error.to_string(),
    }
}

fn byte_of(text: &str, (line, column): (usize, usize)) -> usize {
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let rest = &text[start.min(text.len())..];
    start + rest.chars().take(column).map(char::len_utf8).sum::<usize>()
}

fn place_of(text: &str, byte: usize) -> (usize, usize) {
    let before = &text[..byte];
    let line = before.matches('\n').count();
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count());
    (line, column)
}

fn word_at(text: &str, cursor: (usize, usize)) -> Option<String> {
    let line = text.lines().nth(cursor.0)?;
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = cursor.1.min(chars.len().saturating_sub(1));
    if !chars.get(at).is_some_and(is_word) {
        return None;
    }
    let start = chars[..at].iter().rev().take_while(|c| is_word(c)).count();
    let end = chars[at..].iter().take_while(|c| is_word(c)).count();
    Some(chars[at - start..at + end].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_search_keeps_why_and_says_it_as_vim_does() {
        let invalid = substitute("s/(/x/", "a\n", 0);
        let Some(Err(Note::InvalidPattern { pattern, detail })) = &invalid else {
            panic!("{invalid:?}")
        };
        assert_eq!(pattern, "(");
        assert!(!detail.is_empty());
        assert_eq!(
            find("zz", "a\n", (0, 0), true, 1),
            Err(Note::PatternNotFound("zz".into()))
        );
        assert!(matches!(
            find("(", "a\n", (0, 0), true, 1),
            Err(Note::InvalidPattern { .. })
        ));
        assert_eq!(substitute("s/a/b/", "a\n", 0).map(Result::ok), Some(Some("b\n".into())));
        assert_eq!(
            substitute("s/q/z/", "a\n", 0),
            Some(Err(Note::PatternNotFound("q".into())))
        );
    }

    #[test]
    fn a_match_spans_from_its_first_character_to_after_its_last_and_counts_repeat() {
        let text = "alpha\nbeta\ngamma beta\n";
        let first = Found {
            start: (1, 0),
            end: (1, 4),
        };
        assert_eq!(find("beta", text, (0, 0), true, 1), Ok(first));
        assert_eq!(find("beta", text, (0, 0), true, 2).map(|f| f.start), Ok((2, 6)));
        assert_eq!(
            find("beta", text, (0, 0), true, 3),
            Ok(first),
            "the search wraps around"
        );
    }
}
