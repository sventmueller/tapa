//! Colours, bracket pairs and folds of a document by lines.
//!
//! splat's `tokens` lexes a whole text with no state to resume from. This module lexes one line at a time and keeps
//! the state each line starts in: inside a tag, and inside a block comment or a string that runs on from an earlier
//! line. Such a state is kept as the text that opened it, which in front of the next line makes `tokens` start there
//! inside the same construct. After an edit only the edited lines are lexed again, and the lines below them only
//! until one starts in the state it started in before; a line far below anything lexed so far is lexed from a guessed
//! state first and checked once the lexing reaches it. Every line then gets what `tokens` gives for the whole text,
//! except that a lone CR ends a line here, as it does for the editor and for language servers.
//!
//! Markdown is not lexed by lines: its colours come from splat's parse of the whole text (`markdown`), and the lexing by
//! lines only finds its brackets.

mod folding;
mod language;
mod markdown;

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

pub use folding::Fold;
pub use language::Language;
pub use markdown::{Style, Styled, Token};
pub use splat_markdown::code::CodeToken;

use crate::edit::Change;
use crate::text::Text;

/// How many lines in front of a range asked for are lexed to know its state; beyond that the state is guessed.
const CATCH_UP_LINES: usize = 5_000;
/// How long the first line of a construct may be before its opening is shortened to its first and last characters.
const LONG_OPENING: usize = 256;
/// How much of the start of a construct a shortened opening keeps.
const OPENING_HEAD: usize = 16;
/// Lines lexed between two looks at the clock.
const LINES_PER_STEP: usize = 256;
/// How far a search for the partner of a bracket goes, in lines.
#[cfg(test)]
pub(crate) const BRACKET_LINES: usize = 20_000;
/// Lines longer than this, as minified code has them, stay without colours and brackets and hand on the state they
/// start in, so that a keystroke in one costs no more than in a short line; VS Code draws the line at the same length.
pub(crate) const LONGEST_LEXED_LINE: usize = 20_000;

/// In front of a line, puts `tokens` of a markup language inside a tag.
const IN_TAG: &str = "<\n";
/// In front of an opening, makes `tokens` see a line that already holds something before it, which decides whether
/// a YAML or TOML key may follow once the construct closes; it is neither a token nor whitespace.
const NOT_AT_LINE_START: &str = "\u{1} ";

/// A coloured stretch of one line, in UTF-16 columns of that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// The first column.
    pub start: u32,
    /// The column after the last.
    pub end: u32,
    /// What the stretch is.
    pub kind: CodeToken,
}

/// A bracket outside strings and comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Bracket {
    pub(crate) utf16: u32,
    pub(crate) ch: u8,
}

/// Two brackets that belong together, as UTF-16 offsets into the text.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BracketPair {
    /// The opening bracket.
    pub(crate) open: usize,
    /// The closing bracket.
    pub(crate) close: usize,
}

/// What a line starts inside of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct State {
    in_tag: bool,
    /// The text that opened a construct still open, ending in a line break.
    open: Option<Arc<str>>,
}

#[derive(Debug, Clone)]
struct Line {
    start: State,
    end: State,
    spans: Box<[Span]>,
    brackets: Box<[Bracket]>,
}

/// The lexed lines of a document; lines `..frontier` are lexed from their true state.
#[derive(Debug, Clone)]
pub(crate) struct Syntax {
    language: Option<Language>,
    markup: bool,
    lines: Vec<Option<Line>>,
    frontier: usize,
    input: String,
    markdown: Option<markdown::Markdown>,
    indents: folding::Indents,
}

/// Lines `first..first + removed` replaced by `added` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineEdit {
    pub(crate) first: usize,
    pub(crate) removed: usize,
    pub(crate) added: usize,
}

/// Makes `edits` in `lines` one after the other, every line an edit puts in being `None`, as one splice of the vector
/// per edit would; while the edits run from the end towards the start, as those of a whole new text do, each line that
/// stays moves once rather than once per edit in front of it.
pub(crate) fn splice_lines<T>(lines: &mut Vec<Option<T>>, edits: &[LineEdit]) {
    // The lines are those of `lines` followed by those of `after` from its last to its first.
    let mut after: Vec<Option<T>> = Vec::new();
    for edit in edits {
        let end = (edit.first + edit.removed).min(lines.len() + after.len());
        let start = edit.first.min(end);
        if start > lines.len() {
            lines.extend(after.drain(..).rev());
        }
        let from_after = end.saturating_sub(lines.len());
        after.truncate(after.len() - from_after);
        if end < lines.len() {
            after.extend(lines.drain(end..).rev());
        }
        lines.truncate(start);
        after.extend((0..edit.added).map(|_| None));
    }
    lines.extend(after.into_iter().rev());
}

/// The lines whose colours or brackets changed.
#[derive(Default)]
struct Changed(Option<Range<usize>>);

impl Changed {
    fn add(&mut self, line: usize) {
        self.0 = Some(match self.0.take() {
            None => line..line + 1,
            Some(range) => range.start.min(line)..range.end.max(line + 1),
        });
    }
}

impl Syntax {
    pub(crate) fn new(language: Option<Language>, line_count: usize) -> Syntax {
        let markup =
            language.is_some_and(|l| splat_markdown::code::tokens(l.name(), "<a") == [(1..2, CodeToken::Keyword)]);
        Syntax {
            language,
            markup,
            lines: vec![None; line_count],
            frontier: 0,
            input: String::new(),
            markdown: language
                .filter(|l| l.is_markdown())
                .map(|_| markdown::Markdown::new(line_count)),
            indents: folding::Indents::default(),
        }
    }

    pub(crate) fn language(&self) -> Option<Language> {
        self.language
    }

    /// The lines of `edits` were replaced, one edit after the other, by the text `changes` made.
    pub(crate) fn edited(&mut self, edits: &[LineEdit], changes: &[Change]) {
        let Some(first) = edits.iter().map(|edit| edit.first).min() else {
            return;
        };
        splice_lines(&mut self.lines, edits);
        self.frontier = self.frontier.min(first);
        if let Some(markdown) = &mut self.markdown {
            markdown.edited(edits, changes);
        }
        self.indents.edited(edits);
    }

    /// The folds by indentation of `text`, which is the text the edits so far led to.
    pub(crate) fn folds(&mut self, text: &Text, tab_width: usize) -> Vec<Fold> {
        self.indents.folds(text, tab_width)
    }

    pub(crate) fn spans(&self, line: usize) -> &[Span] {
        self.lines
            .get(line)
            .and_then(Option::as_ref)
            .map_or(&[], |line| &line.spans)
    }

    pub(crate) fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub(crate) fn styles(&self, line: usize) -> &[Styled] {
        self.markdown.as_ref().map_or(&[], |m| m.styles(line))
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.frontier >= self.lines.len() && self.markdown.as_ref().is_none_or(markdown::Markdown::is_complete)
    }

    /// Lexes what `lines` needs, and returns the lines whose colours changed. Catching up on the lines above them
    /// stops at `deadline`, and those of `lines` it did not reach are lexed from a guessed state.
    pub(crate) fn highlight(
        &mut self,
        text: &Text,
        lines: Range<usize>,
        deadline: Option<Instant>,
    ) -> Option<Range<usize>> {
        let end = lines.end.min(self.lines.len());
        let start = lines.start.min(end);
        if start == end {
            return None;
        }
        let mut changed = Changed::default();
        if start <= self.frontier.saturating_add(CATCH_UP_LINES) {
            self.advance(text, end, &mut changed, deadline);
        }
        let first = self.frontier.max(start);
        if first < end {
            let known = (first == self.frontier).then(|| first.checked_sub(1)).flatten();
            let mut state = match known {
                Some(previous) => self.lines[previous].as_ref().map(|l| l.end.clone()),
                None => self.lines[first].as_ref().map(|l| l.start.clone()),
            }
            .unwrap_or_default();
            for n in first..end {
                state = self.relex(text, n, state, &mut changed);
            }
        }
        if let Some(markdown) = &mut self.markdown {
            markdown.highlight(text, start..end, &mut changed);
        }
        changed.0
    }

    /// Lexes on below the lines known until `deadline`, and returns the lines whose colours changed.
    pub(crate) fn highlight_until(&mut self, text: &Text, deadline: Instant) -> Option<Range<usize>> {
        let mut changed = Changed::default();
        self.advance(text, self.lines.len(), &mut changed, Some(deadline));
        if let Some(markdown) = &mut self.markdown {
            markdown.highlight_until(text, deadline, &mut changed);
        }
        changed.0
    }

    fn advance(&mut self, text: &Text, until: usize, changed: &mut Changed, deadline: Option<Instant>) {
        while self.frontier < until {
            let step_end = until.min(self.frontier + LINES_PER_STEP);
            while self.frontier < step_end {
                let n = self.frontier;
                let state = match n.checked_sub(1) {
                    None => State::default(),
                    Some(previous) => self.lines[previous].as_ref().map(|l| l.end.clone()).unwrap_or_default(),
                };
                self.relex(text, n, state, changed);
                self.frontier += 1;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return;
            }
        }
    }

    /// Lexes line `n` from `state` unless it already was, and returns the state the next line starts in.
    fn relex(&mut self, text: &Text, n: usize, state: State, changed: &mut Changed) -> State {
        if let Some(line) = &self.lines[n]
            && line.start == state
        {
            return line.end.clone();
        }
        let line = if text.line_bytes(n).is_some_and(|bytes| bytes.len() > LONGEST_LEXED_LINE) {
            Line {
                start: state.clone(),
                end: state,
                spans: Box::default(),
                brackets: Box::default(),
            }
        } else {
            let content = text.line(n).unwrap_or_default();
            lex(self.language, self.markup, &mut self.input, &content, state)
        };
        let end = line.end.clone();
        let same = self.lines[n]
            .as_ref()
            .is_some_and(|old| old.spans == line.spans && old.brackets == line.brackets);
        if !same {
            changed.add(n);
        }
        self.lines[n] = Some(line);
        end
    }

    /// The bracket at UTF-16 offset `offset` or right before it, and its partner.
    #[cfg(test)]
    pub(crate) fn matching_bracket(&mut self, text: &Text, offset: usize) -> Option<BracketPair> {
        let byte = text.utf16_to_byte(offset).ok()?;
        let line = text.line_of(byte);
        self.highlight(text, line..line + 1, None);
        let line_start = text.byte_to_utf16(text.line_bytes(line)?.start).ok()?;
        let column = u32::try_from(offset - line_start).ok()?;
        let brackets = &self.lines[line].as_ref()?.brackets;
        let index = brackets
            .iter()
            .position(|b| b.utf16 == column)
            .or_else(|| brackets.iter().position(|b| b.utf16 + 1 == column))?;
        let bracket = brackets[index];
        let (open, close) = pair_of(bracket.ch)?;
        let forward = bracket.ch == open;
        let partner = if forward { close } else { open };
        let mut depth = 0;
        let rest = if forward {
            &brackets[index + 1..]
        } else {
            &brackets[..index]
        };
        let found = match walk(rest, forward, bracket.ch, partner, &mut depth) {
            Some(column) => Some((line, column)),
            None if forward => self.walk_lines(
                text,
                line + 1..self.lines.len().min(line + BRACKET_LINES),
                true,
                bracket.ch,
                partner,
                &mut depth,
            ),
            None => self.walk_lines(
                text,
                line.saturating_sub(BRACKET_LINES)..line,
                false,
                bracket.ch,
                partner,
                &mut depth,
            ),
        };
        let (other_line, other_column) = found?;
        let here = line_start + bracket.utf16 as usize;
        let there = text.byte_to_utf16(text.line_bytes(other_line)?.start).ok()? + other_column as usize;
        Some(if forward {
            BracketPair {
                open: here,
                close: there,
            }
        } else {
            BracketPair {
                open: there,
                close: here,
            }
        })
    }

    /// Looks for the partner through `lines`, in the search's direction, lexing them a step at a time.
    #[cfg(test)]
    fn walk_lines(
        &mut self,
        text: &Text,
        lines: Range<usize>,
        forward: bool,
        same: u8,
        partner: u8,
        depth: &mut usize,
    ) -> Option<(usize, u32)> {
        let mut rest = lines;
        while !rest.is_empty() {
            let step = if forward {
                rest.start..rest.end.min(rest.start + LINES_PER_STEP)
            } else {
                rest.end.saturating_sub(LINES_PER_STEP).max(rest.start)..rest.end
            };
            self.highlight(text, step.clone(), None);
            let mut order: Vec<usize> = step.clone().collect();
            if !forward {
                order.reverse();
            }
            for n in order {
                if let Some(column) = walk(&self.lines[n].as_ref()?.brackets, forward, same, partner, depth) {
                    return Some((n, column));
                }
            }
            rest = if forward {
                step.end..rest.end
            } else {
                rest.start..step.start
            };
        }
        None
    }
}

/// The column of the partner among `brackets`, walked in the search's direction; `depth` counts the brackets like
/// the one searched from that are still open, across lines.
#[cfg(test)]
fn walk(brackets: &[Bracket], forward: bool, same: u8, partner: u8, depth: &mut usize) -> Option<u32> {
    let mut visit = |b: &Bracket| {
        if b.ch == same {
            *depth += 1;
        } else if b.ch == partner {
            if *depth == 0 {
                return Some(b.utf16);
            }
            *depth -= 1;
        }
        None
    };
    if forward {
        brackets.iter().find_map(&mut visit)
    } else {
        brackets.iter().rev().find_map(&mut visit)
    }
}

#[cfg(test)]
fn pair_of(ch: u8) -> Option<(u8, u8)> {
    match ch {
        b'(' | b')' => Some((b'(', b')')),
        b'[' | b']' => Some((b'[', b']')),
        b'{' | b'}' => Some((b'{', b'}')),
        _ => None,
    }
}

fn break_len(line: &str) -> usize {
    if line.ends_with("\r\n") {
        2
    } else {
        usize::from(line.ends_with(['\n', '\r']))
    }
}

/// Lexes `content`, a line with its line break, from `start`.
fn lex(language: Option<Language>, markup: bool, input: &mut String, content: &str, start: State) -> Line {
    input.clear();
    if start.in_tag {
        input.push_str(IN_TAG);
    }
    if let Some(open) = &start.open {
        input.push_str(open);
    }
    let prefix = input.len();
    input.push_str(content);
    if !content.ends_with('\n') {
        input.push('\n');
    }
    let content_end = prefix + content.len() - break_len(content);
    let tokens = language.map_or_else(Vec::new, |l| splat_markdown::code::tokens(l.name(), input));

    let quiet = |kind: CodeToken| matches!(kind, CodeToken::String | CodeToken::Comment);
    let mut in_tag = start.in_tag;
    let mut brackets = Vec::new();
    let mut spans = Vec::with_capacity(tokens.len());
    let mut columns = Columns::new(&input[prefix..content_end]);
    let mut next = tokens.iter().peekable();
    for (at, byte) in input.as_bytes()[prefix..content_end].iter().enumerate() {
        let at = prefix + at;
        while next.peek().is_some_and(|(range, _)| range.end <= at) {
            next.next();
        }
        if next
            .peek()
            .is_some_and(|(range, kind)| range.start <= at && quiet(*kind))
        {
            continue;
        }
        match byte {
            b'(' | b')' | b'[' | b']' | b'{' | b'}' => brackets.push(Bracket {
                utf16: columns.at(at - prefix),
                ch: *byte,
            }),
            b'<' if markup => in_tag = true,
            b'>' if markup => in_tag = false,
            _ => {}
        }
    }
    let mut columns = Columns::new(&input[prefix..content_end]);
    for (range, kind) in &tokens {
        let (from, to) = (range.start.max(prefix), range.end.min(content_end));
        if from < to {
            spans.push(Span {
                start: columns.at(from - prefix),
                end: columns.at(to - prefix),
                kind: *kind,
            });
        }
    }

    let open = match tokens.last() {
        Some((range, _)) if range.end == input.len() && range.start < prefix => start.open.clone(),
        Some((range, kind)) if range.end == input.len() => {
            let at_line_start = line_start_before(input, &tokens, range.start);
            Some(opening(language, in_tag, at_line_start, &input[range.start..], *kind))
        }
        _ => None,
    };
    Line {
        start,
        end: State { in_tag, open },
        spans: spans.into_boxed_slice(),
        brackets: brackets.into_boxed_slice(),
    }
}

/// Whether splat's lexer still counts `at` as the start of its line: nothing but blanks, strings and comments lie
/// between it and the last line break outside a token.
fn line_start_before(input: &str, tokens: &[(Range<usize>, CodeToken)], at: usize) -> bool {
    let mut line_start = true;
    let mut next = tokens.iter().peekable();
    for (i, ch) in input[..at].char_indices() {
        while next.peek().is_some_and(|(range, _)| range.end <= i) {
            next.next();
        }
        if let Some((range, kind)) = next.peek()
            && range.start <= i
        {
            if !matches!(kind, CodeToken::String | CodeToken::Comment) {
                line_start = false;
            }
            continue;
        }
        if ch == '\n' {
            line_start = true;
        } else if !ch.is_whitespace() {
            line_start = false;
        }
    }
    line_start
}

/// What to put in front of the next line so that `tokens` starts inside the construct `text` opens, which runs to
/// the end of the lexed line: the construct's first line, or for a long one its first and last characters, if those
/// still open the same kind of construct.
fn opening(language: Option<Language>, in_tag: bool, at_line_start: bool, text: &str, kind: CodeToken) -> Arc<str> {
    let lead = if at_line_start { "" } else { NOT_AT_LINE_START };
    if text.len() > LONG_OPENING
        && let Some(language) = language
    {
        let mut head = OPENING_HEAD;
        while !text.is_char_boundary(head) {
            head -= 1;
        }
        let body = &text[..text.len() - 1];
        let tail = body.char_indices().next_back().map_or(body.len(), |(i, _)| i).max(head);
        let short = format!("{lead}{}{}\n", &text[..head], &body[tail..]);
        let tag = if in_tag { IN_TAG } else { "" };
        let probe = format!("{tag}{short}");
        let opens = splat_markdown::code::tokens(language.name(), &probe)
            .last()
            .is_some_and(|(range, k)| range.start == tag.len() + lead.len() && range.end == probe.len() && *k == kind);
        if opens {
            return short.into();
        }
    }
    format!("{lead}{text}").into()
}

/// UTF-16 columns of byte offsets into a line, asked for in increasing order.
struct Columns<'a> {
    line: &'a str,
    byte: usize,
    utf16: u32,
}

impl<'a> Columns<'a> {
    fn new(line: &'a str) -> Columns<'a> {
        Columns {
            line,
            byte: 0,
            utf16: 0,
        }
    }

    fn at(&mut self, byte: usize) -> u32 {
        if byte < self.byte {
            *self = Columns::new(self.line);
        }
        let units: usize = self.line[self.byte..byte].chars().map(char::len_utf16).sum();
        self.utf16 = self.utf16.saturating_add(u32::try_from(units).unwrap_or(u32::MAX));
        self.byte = byte;
        self.utf16
    }
}

#[cfg(test)]
mod tests;
